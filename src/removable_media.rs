//! Removable-drive detection, per-card fingerprinting, and safe-eject.
//!
//! Built for the planned memory-card ingest feature; not wired into the UI yet.
//! The fingerprint scheme (and its `nearest_bucket_gb` rounding) was validated against a
//! standalone Python prototype across multiple PCs/readers before being ported here — the
//! two must keep producing identical signatures/friendly names for the same card.
#![allow(dead_code)]

use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use std::ffi::c_void;

#[link(name = "kernel32")]
extern "system" {
    fn GetLogicalDrives() -> u32;
    fn GetDriveTypeW(lpRootPathName: *const u16) -> u32;
    fn GetVolumeInformationW(
        lpRootPathName: *const u16,
        lpVolumeNameBuffer: *mut u16,
        nVolumeNameSize: u32,
        lpVolumeSerialNumber: *mut u32,
        lpMaximumComponentLength: *mut u32,
        lpFileSystemFlags: *mut u32,
        lpFileSystemNameBuffer: *mut u16,
        nFileSystemNameSize: u32,
    ) -> i32;
    fn GetDiskFreeSpaceExW(
        lpDirectoryName: *const u16,
        lpFreeBytesAvailable: *mut u64,
        lpTotalNumberOfBytes: *mut u64,
        lpTotalNumberOfFreeBytes: *mut c_void,
    ) -> i32;
    fn CreateFileW(
        lpFileName: *const u16,
        dwDesiredAccess: u32,
        dwShareMode: u32,
        lpSecurityAttributes: *mut c_void,
        dwCreationDisposition: u32,
        dwFlagsAndAttributes: u32,
        hTemplateFile: *mut c_void,
    ) -> *mut c_void;
    fn DeviceIoControl(
        hDevice: *mut c_void,
        dwIoControlCode: u32,
        lpInBuffer: *const c_void,
        nInBufferSize: u32,
        lpOutBuffer: *mut c_void,
        nOutBufferSize: u32,
        lpBytesReturned: *mut u32,
        lpOverlapped: *mut c_void,
    ) -> i32;
    fn CloseHandle(hObject: *mut c_void) -> i32;
}

const DRIVE_REMOVABLE: u32 = 2;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const FSCTL_LOCK_VOLUME: u32 = 0x0009_0018;
const FSCTL_DISMOUNT_VOLUME: u32 = 0x0009_0020;
const IOCTL_STORAGE_MEDIA_REMOVAL: u32 = 0x002D_4804;
const IOCTL_STORAGE_EJECT_MEDIA: u32 = 0x002D_4808;

/// A fingerprint for a card's own filesystem — stable across reformatting-free reinsertion,
/// across readers, and across PCs. Deliberately excludes the reader's hardware/device serial
/// (`IOCTL_STORAGE_QUERY_PROPERTY`), which testing showed is tied to the reader slot, not the
/// card: swapping cards in the same slot reports an identical value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardFingerprint {
    pub drive_letter: char,
    pub label: String,
    pub volume_serial: u32,
    pub fs_type: String,
    pub total_bytes: u64,
    /// First 16 hex chars of sha256(volume_serial|total_bytes|fs_type). Opaque; use this (not
    /// `friendly_name`) for identity/dedup logic — the friendly name is a lossy display label.
    pub signature: String,
    /// e.g. "64GB_88c252" — a human-glanceable label, not a unique key.
    pub friendly_name: String,
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Marketed capacities cluster around round-20 numbers (500, 1000) or powers of two
/// (64, 128, 512). Measured/usable capacity is always a bit under the marketed number, so
/// bucket up (ceiling) to whichever of the two is the closer fit from above — the label
/// never undersells the card.
fn nearest_bucket_gb(size_gb: f64) -> u64 {
    if size_gb <= 0.0 {
        return 0;
    }
    let ceil_20 = (size_gb / 20.0).ceil() as u64 * 20;
    let ceil_pow2 = 2u64.pow(size_gb.log2().ceil() as u32);
    ceil_20.min(ceil_pow2)
}

/// All currently-mounted removable drive letters (USB flash drives, SD/XQD/etc. card readers).
pub fn list_removable_drives() -> Vec<char> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| (b'A' + i as u8) as char)
        .filter(|&letter| {
            let root = to_wide(&format!("{letter}:\\"));
            (unsafe { GetDriveTypeW(root.as_ptr()) }) == DRIVE_REMOVABLE
        })
        .collect()
}

/// Reads the fingerprint for a removable drive. Returns `None` if the drive letter isn't a
/// removable drive, or the media isn't ready (e.g. an empty reader slot).
pub fn fingerprint(drive_letter: char) -> Option<CardFingerprint> {
    let root = format!("{drive_letter}:\\");
    let root_wide = to_wide(&root);

    if unsafe { GetDriveTypeW(root_wide.as_ptr()) } != DRIVE_REMOVABLE {
        return None;
    }

    let mut label_buf = [0u16; 261];
    let mut fs_buf = [0u16; 261];
    let mut serial: u32 = 0;
    let ok = unsafe {
        GetVolumeInformationW(
            root_wide.as_ptr(),
            label_buf.as_mut_ptr(),
            label_buf.len() as u32,
            &mut serial,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs_buf.as_mut_ptr(),
            fs_buf.len() as u32,
        )
    };
    if ok == 0 {
        return None;
    }

    let label_len = label_buf.iter().position(|&c| c == 0).unwrap_or(0);
    let label = String::from_utf16_lossy(&label_buf[..label_len]);
    let fs_len = fs_buf.iter().position(|&c| c == 0).unwrap_or(0);
    let fs_type = String::from_utf16_lossy(&fs_buf[..fs_len]);

    let mut total: u64 = 0;
    let ok = unsafe {
        GetDiskFreeSpaceExW(root_wide.as_ptr(), std::ptr::null_mut(), &mut total, std::ptr::null_mut())
    };
    if ok == 0 {
        // Media can be pulled between the GetVolumeInformationW call above and here; don't
        // let a transient failure silently degrade the fingerprint to a "0GB" signature.
        return None;
    }
    let total_bytes = total;

    let canonical = format!("{serial:08X}|{total_bytes}|{fs_type}");
    let digest = Sha256::digest(canonical.as_bytes());
    let full_hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let signature = full_hex[..16].to_string();

    let size_gb_rounded = nearest_bucket_gb(total_bytes as f64 / 1_000_000_000.0);
    let friendly_name = format!("{size_gb_rounded}GB_{}", &signature[..6]);

    Some(CardFingerprint {
        drive_letter,
        label,
        volume_serial: serial,
        fs_type,
        total_bytes,
        signature,
        friendly_name,
    })
}

/// Locks, dismounts, and ejects a removable volume — the same sequence Windows uses for
/// "Safely Remove Hardware". A `FSCTL_LOCK_VOLUME` failure means something still has a file
/// open on the drive; surface that to the user rather than a generic error.
pub fn safe_eject(drive_letter: char) -> Result<()> {
    let root = to_wide(&format!("{drive_letter}:\\"));
    if (unsafe { GetDriveTypeW(root.as_ptr()) }) != DRIVE_REMOVABLE {
        return Err(anyhow!("{drive_letter}:\\ is not a removable drive"));
    }

    let path = format!("\\\\.\\{drive_letter}:");
    let path_wide = to_wide(&path);
    let handle = unsafe {
        CreateFileW(
            path_wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle as isize == -1 {
        return Err(anyhow!("CreateFileW failed: {}", std::io::Error::last_os_error()));
    }

    let result = (|| {
        ioctl(handle, FSCTL_LOCK_VOLUME)
            .map_err(|e| anyhow!("volume is in use, close open files first ({e})"))?;
        ioctl(handle, FSCTL_DISMOUNT_VOLUME).map_err(|e| anyhow!("dismount failed: {e}"))?;

        let allow_removal: u8 = 0;
        let mut bytes_returned: u32 = 0;
        unsafe {
            DeviceIoControl(
                handle,
                IOCTL_STORAGE_MEDIA_REMOVAL,
                &allow_removal as *const _ as *const c_void,
                1,
                std::ptr::null_mut(),
                0,
                &mut bytes_returned,
                std::ptr::null_mut(),
            );
        }

        // Not all readers implement media eject (e.g. some SD/XQD reader hardware); a failure
        // here still leaves the volume safely dismounted, so treat it as a soft success.
        let _ = ioctl(handle, IOCTL_STORAGE_EJECT_MEDIA);

        Ok(())
    })();

    unsafe { CloseHandle(handle) };
    result
}

fn ioctl(handle: *mut c_void, code: u32) -> std::result::Result<(), std::io::Error> {
    let mut bytes_returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            code,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &mut bytes_returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::nearest_bucket_gb;

    #[test]
    fn buckets_to_whichever_of_round_20_or_power_of_2_is_closer_from_above() {
        assert_eq!(nearest_bucket_gb(63.8), 64);
        assert_eq!(nearest_bucket_gb(108.7), 120);
        assert_eq!(nearest_bucket_gb(510.1), 512);
        assert_eq!(nearest_bucket_gb(504.0), 512);
    }

    #[test]
    fn never_undersells_the_card() {
        for tenth in 1..2000 {
            let size_gb = tenth as f64 / 10.0;
            assert!(nearest_bucket_gb(size_gb) as f64 >= size_gb);
        }
    }

    #[test]
    fn zero_or_negative_is_zero() {
        assert_eq!(nearest_bucket_gb(0.0), 0);
        assert_eq!(nearest_bucket_gb(-5.0), 0);
    }

    // Manual check against real hardware: `cargo test -- --ignored --nocapture manual_print`.
    #[test]
    #[ignore]
    fn manual_print_removable_drives() {
        for letter in super::list_removable_drives() {
            match super::fingerprint(letter) {
                Some(fp) => println!("{fp:?}"),
                None => println!("{letter}:\\ not ready / no media"),
            }
        }
    }
}
