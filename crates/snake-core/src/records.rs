use std::collections::HashMap;

/// Best length per record key (see [`crate::config::Rules::record_key`]).
#[derive(Default)]
pub struct Records {
    map: HashMap<String, u32>,
}

impl Records {
    /// Parses `key<TAB>best` lines, skipping malformed ones.
    pub fn from_text(text: &str) -> Self {
        let mut map = HashMap::new();
        for line in text.lines() {
            if let Some((key, value)) = line.rsplit_once('\t')
                && let Ok(n) = value.trim().parse()
            {
                map.insert(key.to_string(), n);
            }
        }
        Self { map }
    }

    pub fn to_text(&self) -> String {
        let mut lines: Vec<String> = self.map.iter().map(|(k, v)| format!("{k}\t{v}")).collect();
        lines.sort();
        lines.join("\n") + "\n"
    }

    pub fn get(&self, key: &str) -> u32 {
        self.map.get(key).copied().unwrap_or(0)
    }

    /// Records `value` if it beats the current best; returns whether it did.
    pub fn update(&mut self, key: &str, value: u32) -> bool {
        if value > self.get(key) {
            self.map.insert(key.to_string(), value);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trip() {
        let mut records = Records::default();
        assert!(records.update("size:30x20 tick:150", 12));
        assert!(!records.update("size:30x20 tick:150", 7));
        assert!(records.update("size:10x8 tick:50", 3));
        let restored = Records::from_text(&records.to_text());
        assert_eq!(restored.get("size:30x20 tick:150"), 12);
        assert_eq!(restored.get("size:10x8 tick:50"), 3);
    }

    #[test]
    fn skips_malformed_lines() {
        let records = Records::from_text("good\t5\nno tab here\nbad\tvalue\n\n");
        assert_eq!(records.get("good"), 5);
        assert_eq!(records.get("bad"), 0);
    }
}
