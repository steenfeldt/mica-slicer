// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The model library, read-only: what Mica last saved in its index
//! (`mica_library::read_view`), re-read when the index file changes. The dashboard never
//! opens the index itself, so a Mica window using the same settings folder keeps scanning
//! and owning it. Searching uses the library's own filter (`mica_library::query`).

use crate::fleet::{colour_json, lock};
use mica_library::{Filter, Kind, Model, Snapshot, Sort, query};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

/// The index is checked for changes at most this often.
const RELOAD_EVERY: Duration = Duration::from_secs(5);
/// Results per page.
const MAX_LIMIT: usize = 200;

pub struct Library {
    data_dir: PathBuf,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    checked: Option<Instant>,
    stamp: Option<(u64, SystemTime)>,
    view: Arc<View>,
}

#[derive(Default)]
struct View {
    snapshot: Snapshot,
    haystacks: Vec<String>,
    error: Option<String>,
}

fn stamp(path: &Path) -> Option<(u64, SystemTime)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl Library {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            state: Mutex::default(),
        }
    }

    fn view(&self) -> Arc<View> {
        let mut st = lock(&self.state);
        if st.checked.is_none_or(|t| t.elapsed() >= RELOAD_EVERY) {
            st.checked = Some(Instant::now());
            let now = stamp(&self.data_dir.join("index.redb"));
            if now != st.stamp || st.view.error.is_some() {
                st.stamp = now;
                st.view = Arc::new(match mica_library::read_view(&self.data_dir) {
                    Ok(snapshot) => View {
                        haystacks: query::haystacks(&snapshot.models),
                        snapshot,
                        error: None,
                    },
                    Err(e) => View {
                        error: Some(format!("Mica could not read the library: {e}")),
                        ..View::default()
                    },
                });
            }
        }
        st.view.clone()
    }

    /// One page of models matching `text` (all words), newest first; `gcode` keeps only
    /// G-code files.
    pub fn search(&self, text: &str, gcode: bool, offset: usize, limit: usize) -> Value {
        let view = self.view();
        let filter = Filter {
            text: text.to_string(),
            kinds: if gcode {
                BTreeSet::from([Kind::Gcode, Kind::Bgcode])
            } else {
                BTreeSet::new()
            },
            ..Filter::default()
        };
        let models = &view.snapshot.models;
        let hits = query::apply(
            models,
            &view.haystacks,
            &filter,
            Sort::Modified,
            true,
            now_secs(),
        );
        let limit = limit.clamp(1, MAX_LIMIT);
        let page: Vec<Value> = hits
            .iter()
            .skip(offset)
            .take(limit)
            .map(|&i| model_json(&models[i]))
            .collect();
        json!({
            "models": page,
            "total": hits.len(),
            "count": models.len(),
            "folders": view.snapshot.folders.len(),
            "error": view.error,
        })
    }

    /// A model by its content hash.
    pub fn model(&self, id: &str) -> Option<Model> {
        self.view()
            .snapshot
            .models
            .iter()
            .find(|m| m.id == id)
            .cloned()
    }

    /// The cached thumbnail of model `id`: its bytes and content type. Only files in the
    /// library's own thumbnail folder are read.
    pub fn thumbnail(&self, id: &str) -> Option<(Vec<u8>, &'static str)> {
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let path = self.model(id)?.thumbnail?;
        let thumbs = self.data_dir.join("thumbs");
        if path.parent() != Some(thumbs.as_path()) {
            return None;
        }
        let kind = match path.extension()?.to_str()? {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            _ => return None,
        };
        Some((std::fs::read(&path).ok()?, kind))
    }
}

fn model_json(m: &Model) -> Value {
    let primary = m.primary();
    let colours: Vec<Value> = m
        .info
        .colours
        .iter()
        .map(|c| colour_json(Some(c)))
        .filter(|c| !c.is_null())
        .collect();
    json!({
        "id": m.id,
        "name": m.name,
        "kind": m.kind.label(),
        "gcode": m.kind.is_gcode(),
        "folder": primary.folder(),
        "online": m.online(),
        "size": m.size(),
        "modified": m.modified(),
        "thumbnail": m.thumbnail.is_some(),
        "printer": m.info.printer,
        "print_time": m.info.print_time_s,
        "filament_g": m.info.filament_g,
        "dimensions": m.info.size_mm,
        "colours": colours,
        "favorite": m.user.favourite,
    })
}
