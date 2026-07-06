use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::config::data_dir;

pub struct Records {
    map: HashMap<String, u32>,
    path: PathBuf,
}

impl Records {
    pub fn load() -> Self {
        let path = data_dir().join("records");
        let mut map = HashMap::new();
        if let Ok(contents) = fs::read_to_string(&path) {
            for line in contents.lines() {
                if let Some((key, value)) = line.rsplit_once('\t')
                    && let Ok(n) = value.trim().parse()
                {
                    map.insert(key.to_string(), n);
                }
            }
        }
        Self { map, path }
    }

    pub fn get(&self, key: &str) -> u32 {
        self.map.get(key).copied().unwrap_or(0)
    }

    pub fn update(&mut self, key: &str, value: u32) -> bool {
        if value > self.get(key) {
            self.map.insert(key.to_string(), value);
            self.save();
            true
        } else {
            false
        }
    }

    fn save(&self) {
        let mut lines: Vec<String> = self.map.iter().map(|(k, v)| format!("{k}\t{v}")).collect();
        lines.sort();
        let _ = fs::write(&self.path, lines.join("\n") + "\n");
    }
}
