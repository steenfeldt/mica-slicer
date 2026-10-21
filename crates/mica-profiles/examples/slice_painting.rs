// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Slice every printable instance individually at the bed centre through the library API.
//! This also handles multi-plate projects without treating their global plate offsets as
//! printable coordinates. No G-code is emitted; each finished batch is released immediately.
//! cargo run --offline --release -p mica-profiles --example slice_painting -- project.3mf
use mica_core::{
    geometry::{SCALE, polyline_length},
    print::LayerSource,
};
use std::{collections::BTreeMap, path::PathBuf, time::Instant};

fn main() -> Result<(), String> {
    let _timings = mica_core::timing::Report::default();
    let (bundle, _) = mica_profiles::orca::discovery::load_installed(None, "Snapmaker")
        .map_err(|e| e.to_string())?;
    let mut config = bundle
        .single_tool_config(
            "Snapmaker U1 (0.4 nozzle)",
            "0.20 Standard @Snapmaker U1 (0.4 nozzle)",
            "Snapmaker PLA @U1",
            Some("High Temp Plate"),
        )
        .map_err(|e| e.to_string())?;
    config
        .set_raw("wipe_tower", "0")
        .map_err(|e| e.to_string())?;
    for path in std::env::args().skip(1).map(PathBuf::from) {
        let started = Instant::now();
        let loaded = mica_core::input::load(
            std::slice::from_ref(&path),
            (135., 135.),
            false,
            true,
            0,
            &BTreeMap::new(),
            &config,
            &mica_profiles::project::Mapper::default(),
        )?;
        let mut totals = BTreeMap::<usize, f64>::new();
        let mut objects = vec![];
        for mut input in loaded.objects {
            if std::env::var("MICA_PAINTING_OBJECT")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .is_some_and(|id| id != input.object_id)
            {
                continue;
            }
            let t = Instant::now();
            let (min, max) = input.mesh.bounds();
            let shift = [
                135. - (min[0] + max[0]) * 0.5,
                135. - (min[1] + max[1]) * 0.5,
                -min[2],
            ];
            input.mesh.translate(shift);
            if let Some(model) = &mut input.region {
                for volume in &mut model.volumes {
                    volume.mesh.translate(shift);
                }
            }
            let name = input.name.clone();
            let id = input.object_id;
            let painted: usize = input.region.as_ref().map_or(0, |m| {
                m.volumes
                    .iter()
                    .map(|v| v.painting.material.painted_triangles())
                    .sum()
            });
            eprintln!(
                "{} object {id}: prepare {} triangles, {painted} painted",
                path.display(),
                input.mesh.faces.len()
            );
            let object = input.prepare()?;
            eprintln!(
                "{} object {id}: prepared in {:.2?}; generate {} layers",
                path.display(),
                t.elapsed(),
                object.prepared.layer_count()
            );
            let mut by_tool = BTreeMap::<usize, f64>::new();
            for begin in (0..object.prepared.layer_count()).step_by(32) {
                for (layer, regions) in object
                    .prepared
                    .regional_toolpaths(begin..(begin + 32).min(object.prepared.layer_count()))
                {
                    for (i, p) in layer.paths.iter().enumerate() {
                        let tools = regions
                            .get(i)
                            .copied()
                            .flatten()
                            .and_then(|id| object.prepared.region(id))
                            .map_or(&object.tools, |r| &r.tools);
                        let tool = tools.tool(p.role).unwrap_or(0);
                        let diameter = object
                            .config
                            .float_at("filament_diameter", tool)
                            .unwrap_or(1.75);
                        let volume = polyline_length(&p.points) / SCALE * p.flow.mm3_per_mm();
                        *by_tool.entry(tool).or_default() +=
                            volume / (std::f64::consts::PI * diameter * diameter / 4.);
                    }
                }
            }
            for (&tool, &mm) in &by_tool {
                *totals.entry(tool).or_default() += mm;
            }
            eprintln!(
                "{} object {id}: finished {:.2?}",
                path.display(),
                t.elapsed()
            );
            objects.push(serde_json::json!({"id":id,"name":name,"painted_triangles":painted,"layers":object.prepared.layer_count(),"filament_mm":by_tool,"elapsed_s":t.elapsed().as_secs_f64()}));
        }
        println!(
            "{}",
            serde_json::json!({"path":path,"objects":objects,"filament_mm":totals,"elapsed_s":started.elapsed().as_secs_f64()})
        );
    }
    Ok(())
}
