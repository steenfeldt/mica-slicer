// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

#![no_main]

use libfuzzer_sys::fuzz_target;
use mica_core::threemf::read_3mf;
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::SimpleFileOptions};

const STRUCTURED_MODEL: &[u8] = br#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources><object id="1"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/><vertex x="0" y="0" z="1"/></vertices><triangles><triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="1" v3="3"/><triangle v1="0" v2="3" v3="2"/><triangle v1="1" v2="2" v3="3"/></triangles></mesh></object></resources><build><item objectid="1"/></build></model>"#;

fn package(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("[Content_Types].xml", options).unwrap();
    zip.write_all(br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>"#).unwrap();
    for (name, data) in entries {
        zip.start_file(*name, options).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fuzz_target!(|data: &[u8]| {
    // CI also sets libFuzzer's max_len. Keep this guard for direct/reproducer execution.
    if data.len() > 256 * 1024 {
        return;
    }

    let structured;
    let input = if let Some(xml) = data.strip_prefix(b"M3MF") {
        structured = package(&[("3D/3dmodel.model", xml)]);
        structured.as_slice()
    } else if let Some(relationships) = data.strip_prefix(b"MREL") {
        structured = package(&[
            ("_rels/.rels", relationships),
            ("3D/3dmodel.model", STRUCTURED_MODEL),
        ]);
        structured.as_slice()
    } else if let Some(config) = data.strip_prefix(b"MCFG") {
        structured = package(&[
            ("3D/3dmodel.model", STRUCTURED_MODEL),
            ("Metadata/project_settings.config", config),
        ]);
        structured.as_slice()
    } else {
        // Raw mode exercises ZIP framing, central-directory and compression failures.
        data
    };

    let _ = read_3mf(Cursor::new(input));
});
