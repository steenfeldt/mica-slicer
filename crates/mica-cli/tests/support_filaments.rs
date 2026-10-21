// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Supports printed with another filament (docs/support-filaments.md), on a small T: a
//! column with arms that need support.
//!
//! - XL (PrusaSlicer rules): a soluble support filament on the wipe tower's first layer
//!   prints first; the wipe tower refuses normal supports that would need a tool change
//!   (`Print::validate`).
//! - U1 (Orca rules): the prime tower puts the support layers on the object layers
//!   (`normalize_fdm_2`); without a tower, support-only layers print a "Default" base with
//!   the body filament unless "Avoid interface filament for base" is off.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp(ext: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "mica-support-filaments-{}-{}.{ext}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// A binary STL of axis-aligned boxes `(x0, y0, z0, x1, y1, z1)`; overlapping shells are
/// unioned by slicing.
fn boxes(list: &[[f32; 6]]) -> PathBuf {
    let mut triangles: Vec<[[f32; 3]; 3]> = Vec::new();
    for b in list {
        let p = |i: usize| {
            [
                if i & 1 == 0 { b[0] } else { b[3] },
                if i & 2 == 0 { b[1] } else { b[4] },
                if i & 4 == 0 { b[2] } else { b[5] },
            ]
        };
        // Outward winding per face (corner bits: x=1, y=2, z=4).
        for [a, c, d, e] in [
            [0, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ] {
            triangles.push([p(a), p(c), p(d)]);
            triangles.push([p(a), p(d), p(e)]);
        }
    }
    let mut data = vec![0u8; 80];
    data.extend((triangles.len() as u32).to_le_bytes());
    for t in &triangles {
        data.extend([0u8; 12]);
        for v in t {
            for c in v {
                data.extend(c.to_le_bytes());
            }
        }
        data.extend([0u8; 2]);
    }
    let path = temp("stl");
    std::fs::write(&path, data).unwrap();
    path
}

/// The T: a 6 mm column, 12 mm tall, with 30 mm arms on top.
fn tee() -> PathBuf {
    boxes(&[[-3., -3., 0., 3., 3., 12.], [-15., -3., 10., 15., 3., 12.]])
}

/// Two arms at different heights, off the layer grid: a support-only layer prints the low
/// arm's interface next to the high arm's base.
fn steps() -> PathBuf {
    boxes(&[
        [-3., -3., 0., 3., 3., 22.],
        [-15., -3., 20.1, 0., 3., 22.],
        [0., -3., 10.1, 15., 3., 12.],
    ])
}

const XL: &[&str] = &[
    "--printer",
    "Original Prusa XL - 5T Input Shaper 0.4 nozzle",
    "--print",
    "0.20mm SPEED @XLIS 0.4",
    "--filament",
    "Prusament PLA @XLIS",
];
const U1: &[&str] = &[
    "--vendor",
    "Snapmaker",
    "--printer",
    "Snapmaker U1 (0.4 nozzle)",
    "--print",
    "0.20 Standard @Snapmaker U1 (0.4 nozzle)",
    "--filament",
    "Snapmaker PLA @U1",
    "--bed",
    "High Temp Plate",
];

/// `mica slice` of `model`; `Ok(None)` when the profiles are not installed.
fn slice(model: &Path, profile: &[&str], sets: &[&str]) -> Result<Option<String>, String> {
    let out = temp("gcode");
    let mut all: Vec<String> = vec!["slice".into(), model.to_string_lossy().into_owned()];
    all.extend(profile.iter().map(|s| s.to_string()));
    for s in sets {
        all.extend(["--set".to_string(), s.to_string()]);
    }
    all.extend(["-o".into(), out.to_string_lossy().into_owned()]);
    if !mica_testkit::require_profiles_for(&all) {
        return Ok(None);
    }
    match mica_cli::run(&all) {
        Ok(()) => {}
        Err(e) => return Err(e),
    }
    let text = std::fs::read_to_string(&out).unwrap();
    let _ = std::fs::remove_file(&out);
    Ok(Some(text))
}

/// Per layer: the tools that extrude, in order, and per (tool, role) whether it extrudes.
struct Layers {
    tools: Vec<Vec<usize>>,
    role_tools: std::collections::BTreeSet<(usize, String)>,
}

fn layers(g: &str) -> Layers {
    let mut tools: Vec<Vec<usize>> = Vec::new();
    let mut role_tools = std::collections::BTreeSet::new();
    let (mut tool, mut role) = (0usize, String::new());
    for line in g.lines() {
        if line == ";LAYER_CHANGE" {
            tools.push(Vec::new());
        } else if let Some(r) = line.strip_prefix(";TYPE:") {
            role = r.to_string();
        } else if let Some(t) = line
            .strip_prefix('T')
            .and_then(|t| t.split_whitespace().next()?.parse().ok())
        {
            // `T1`, or the XL's `T1 S1 L0 D0`.
            tool = t;
        } else if line.starts_with("G1 X")
            && line.contains(" E")
            && !line.contains(" E-")
            && let Some(layer) = tools.last_mut()
        {
            if layer.last() != Some(&tool) {
                layer.push(tool);
            }
            role_tools.insert((tool, role.clone()));
        }
    }
    Layers { tools, role_tools }
}

const XL_SOLUBLE: &[&str] = &[
    "support_material=1",
    "support_material_auto=1",
    "support_material_style=grid",
    "support_material_contact_distance=0",
    "support_material_synchronize_layers=1",
    "support_material_extruder=2",
    "support_material_interface_extruder=2",
    "filament_soluble=0,1,0,0,0",
    "wipe_tower=1",
];

#[test]
fn xl_soluble_supports_start_the_first_tower_layer() {
    let model = tee();
    let Some(g) = slice(&model, XL, XL_SOLUBLE).unwrap() else {
        return;
    };
    let l = layers(&g);
    // reorder_extruders: the soluble tool prints first on the first layer, then the last
    // tool of each layer starts the next.
    assert_eq!(l.tools[0], [1, 0], "{:?}", &l.tools[..3]);
    assert_eq!(l.tools[1], [0, 1], "{:?}", &l.tools[..3]);
    // Every support path on the soluble tool, the model on the first.
    assert!(l.role_tools.contains(&(1, "Support material".into())));
    assert!(
        l.role_tools
            .contains(&(1, "Support material interface".into()))
    );
    assert!(!l.role_tools.contains(&(0, "Support material".into())));
    assert!(!l.role_tools.contains(&(1, "External perimeter".into())));
    let _ = std::fs::remove_file(&model);
}

#[test]
fn xl_wipe_tower_refuses_supports_it_cannot_print() {
    let model = tee();
    // Soluble (zero gap) normal supports need synchronized layers.
    let unsynchronized: Vec<&str> = XL_SOLUBLE
        .iter()
        .copied()
        .filter(|s| !s.starts_with("support_material_synchronize_layers"))
        .collect();
    match slice(&model, XL, &unsynchronized) {
        Ok(None) => return,
        Ok(Some(_)) => panic!("sliced unsynchronized soluble supports with a wipe tower"),
        Err(e) => assert!(e.contains("synchronized with the object layers"), "{e}"),
    }
    // A gap to the model and an explicit support tool.
    let mut gap: Vec<&str> = XL_SOLUBLE.to_vec();
    gap.push("support_material_contact_distance=0.2");
    let e = slice(&model, XL, &gap).unwrap_err();
    assert!(e.contains("current tool without a tool change"), "{e}");
    // Organic supports are exempt, as upstream.
    let mut organic = gap.clone();
    organic.push("support_material_style=organic");
    assert!(slice(&model, XL, &organic).unwrap().is_some());
    let _ = std::fs::remove_file(&model);
}

const U1_SUPPORT: &[&str] = &[
    "enable_support=1",
    "support_type=normal(auto)",
    "support_style=default",
    "support_filament=0",
    "support_interface_filament=2",
];

#[test]
fn u1_prime_tower_synchronizes_supports_and_avoids_the_interface_filament() {
    let model = steps();
    let with = |extra: &[&str]| {
        let mut sets = U1_SUPPORT.to_vec();
        sets.extend_from_slice(extra);
        slice(&model, U1, &sets).unwrap()
    };
    let Some(tower) = with(&["enable_prime_tower=1"]) else {
        return;
    };
    let no_tower = with(&["enable_prime_tower=0"]).unwrap();
    // Independent support layer heights add support-only layers; the prime tower turns
    // them off (normalize_fdm_2).
    let (tower, no_tower) = (layers(&tower), layers(&no_tower));
    assert!(
        tower.tools.len() < no_tower.tools.len(),
        "{} vs {} layers",
        tower.tools.len(),
        no_tower.tools.len()
    );
    // Without the tower, the support-only layers print their "Default" base with the body
    // filament (support_interface_not_for_body, on by default) ...
    assert!(!no_tower.role_tools.contains(&(1, "Support".into())));
    // ... or, with it off, with the filament cheapest to flush to from the interface's: the
    // interface filament itself.
    let interface_too = layers(
        &with(&[
            "enable_prime_tower=0",
            "support_interface_not_for_body=0",
            "flush_volumes_matrix=0,100,100,100,100,0,100,100,100,100,0,100,100,100,100,0",
        ])
        .unwrap(),
    );
    assert!(interface_too.role_tools.contains(&(1, "Support".into())));
    assert!(
        interface_too
            .role_tools
            .contains(&(1, "Support interface".into()))
    );
    let _ = std::fs::remove_file(&model);
}
