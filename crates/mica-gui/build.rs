// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Windows resources for `mica-gui.exe`: the app icon (Explorer, the taskbar, the installer's
//! Start menu and desktop shortcuts all take icon 0 of the exe) and the version information
//! (Properties > Details, Task Manager's process name).
//!
//! The build writes a compiled resource file (`.res`) itself and hands it to the linker, so
//! no resource compiler (rc.exe / llvm-rc) is needed and the cross build (`cargo xwin`) works
//! like a native one. MSVC targets only: link.exe and lld-link take a `.res` as an input file.

use std::{env, fs, path::PathBuf};

const ICON: &str = "assets/icon/mica.ico";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={ICON}");
    println!("cargo:rerun-if-changed=Cargo.toml");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        return;
    }
    let ico = fs::read(ICON).unwrap_or_else(|e| panic!("{ICON}: {e}"));
    let manifest = fs::read_to_string("Cargo.toml").expect("Cargo.toml");
    let meta = |key: &str| packager_value(&manifest, key).unwrap_or_default();
    let product = meta("product-name");
    let info = VersionInfo {
        version: env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION"),
        strings: vec![
            ("CompanyName", meta("publisher")),
            ("FileDescription", product.clone()),
            ("FileVersion", env::var("CARGO_PKG_VERSION").unwrap()),
            ("InternalName", "mica-gui".into()),
            ("LegalCopyright", meta("copyright")),
            ("OriginalFilename", "mica-gui.exe".into()),
            ("ProductName", product),
            ("ProductVersion", env::var("CARGO_PKG_VERSION").unwrap()),
        ],
    };
    let mut res = Vec::new();
    // A .res file opens with an empty entry.
    push_resource(&mut res, 0, 0, &[]);
    let images = ico_images(&ico);
    for (i, (_, data)) in images.iter().enumerate() {
        push_resource(&mut res, RT_ICON, i as u16 + 1, data);
    }
    push_resource(&mut res, RT_GROUP_ICON, 1, &group_icon(&images));
    push_resource(&mut res, RT_VERSION, 1, &info.encode());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("mica-gui.res");
    fs::write(&out, res).expect("write mica-gui.res");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
/// English (United States).
const LANGUAGE: u16 = 0x0409;
const CODE_PAGE: u16 = 1200; // UTF-16

/// A `key = "value"` line of `[package.metadata.packager]` in this crate's Cargo.toml (kept as
/// the one place for the product name, publisher and copyright).
fn packager_value(manifest: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line == "[package.metadata.packager]";
            continue;
        }
        let Some(rest) = line.strip_prefix(key) else {
            continue;
        };
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        if in_section {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// One resource with an ordinal type and name (the RESOURCEHEADER layout of a .res file).
fn push_resource(res: &mut Vec<u8>, kind: u16, name: u16, data: &[u8]) {
    let language = if kind == 0 { 0 } else { LANGUAGE };
    let flags: u16 = if kind == 0 { 0 } else { 0x1030 }; // MOVEABLE | PURE | DISCARDABLE
    res.extend((data.len() as u32).to_le_bytes());
    res.extend(32u32.to_le_bytes()); // header size
    for word in [0xFFFF, kind, 0xFFFF, name] {
        res.extend(u16::to_le_bytes(word));
    }
    res.extend(0u32.to_le_bytes()); // data version
    res.extend(flags.to_le_bytes());
    res.extend(language.to_le_bytes());
    res.extend(0u32.to_le_bytes()); // version
    res.extend(0u32.to_le_bytes()); // characteristics
    res.extend(data);
    pad4(res);
}

fn pad4(v: &mut Vec<u8>) {
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
}

/// The images of an .ico file: its 16-byte directory entry (without the file offset) and data.
fn ico_images(ico: &[u8]) -> Vec<([u8; 12], Vec<u8>)> {
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes(ico[i..i + 4].try_into().unwrap()) as usize;
    assert!(
        ico.len() > 6 && u16_at(2) == 1,
        "{ICON} is not an icon file"
    );
    (0..u16_at(4) as usize)
        .map(|i| {
            let entry = 6 + 16 * i;
            let (len, offset) = (u32_at(entry + 8), u32_at(entry + 12));
            let head: [u8; 12] = ico[entry..entry + 12].try_into().unwrap();
            (head, ico[offset..offset + len].to_vec())
        })
        .collect()
}

/// GRPICONDIR: the .ico directory with resource IDs in place of file offsets.
fn group_icon(images: &[([u8; 12], Vec<u8>)]) -> Vec<u8> {
    let mut v = Vec::new();
    for word in [0u16, 1, images.len() as u16] {
        v.extend(word.to_le_bytes());
    }
    for (i, (head, _)) in images.iter().enumerate() {
        v.extend(head);
        v.extend((i as u16 + 1).to_le_bytes());
    }
    v
}

struct VersionInfo {
    version: String,
    strings: Vec<(&'static str, String)>,
}

impl VersionInfo {
    /// VS_VERSIONINFO with a fixed part, one string table and the translation.
    fn encode(&self) -> Vec<u8> {
        let mut parts = self
            .version
            .split(['.', '-', '+'])
            .map(|p| p.parse::<u16>().unwrap_or(0));
        let [major, minor, patch] = [(); 3].map(|()| parts.next().unwrap_or(0) as u32);
        let (ms, ls) = ((major << 16) | minor, patch << 16);
        let mut fixed = Vec::new();
        // VS_FIXEDFILEINFO: signature, structure version, file version (MS, LS), product
        // version (MS, LS), flags mask, flags, OS (VOS_NT_WINDOWS32), type (VFT_APP), subtype,
        // date (MS, LS).
        #[rustfmt::skip]
        let dwords = [0xFEEF_04BDu32, 0x1_0000, ms, ls, ms, ls, 0x3F, 0, 0x4_0004, 1, 0, 0, 0];
        for dword in dwords {
            fixed.extend(dword.to_le_bytes());
        }
        let strings = self
            .strings
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(key, value)| {
                let text = utf16z(value);
                block(key, &text, (text.len() / 2) as u16, 1, &[])
            })
            .collect::<Vec<_>>();
        let table_key = format!("{LANGUAGE:04x}{CODE_PAGE:04x}");
        let table = block(&table_key, &[], 0, 1, &strings);
        let string_info = block("StringFileInfo", &[], 0, 1, &[table]);
        let translation = [LANGUAGE.to_le_bytes(), CODE_PAGE.to_le_bytes()].concat();
        let var = block("Translation", &translation, 4, 0, &[]);
        let var_info = block("VarFileInfo", &[], 0, 1, &[var]);
        block(
            "VS_VERSION_INFO",
            &fixed,
            fixed.len() as u16,
            0,
            &[string_info, var_info],
        )
    }
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// One version-information block: length, value length, type, key, value, children (each
/// 32-bit aligned; the length leaves out trailing padding).
fn block(key: &str, value: &[u8], value_len: u16, kind: u16, children: &[Vec<u8>]) -> Vec<u8> {
    let mut v = vec![0, 0];
    v.extend(value_len.to_le_bytes());
    v.extend(kind.to_le_bytes());
    v.extend(utf16z(key));
    if !value.is_empty() {
        pad4(&mut v);
        v.extend(value);
    }
    for child in children {
        pad4(&mut v);
        v.extend(child);
    }
    let len = v.len() as u16;
    v[..2].copy_from_slice(&len.to_le_bytes());
    v
}
