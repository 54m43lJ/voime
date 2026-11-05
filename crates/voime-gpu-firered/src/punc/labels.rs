use std::path::Path;

use anyhow::{bail, Context};

use crate::punc::tokenizer::Piece;

pub struct PuncLabels {
    labels: Vec<String>,
}

impl PuncLabels {
    pub fn load(out_dict: &Path, punc_json: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(out_dict)
            .with_context(|| format!("无法读取标点标签 {}", out_dict.display()))?;
        let mut entries: Vec<Option<String>> = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let (label, id_text) = line
                .rsplit_once(' ')
                .with_context(|| format!("标点标签格式错误 {}：{line}", out_dict.display()))?;
            let id: usize = id_text
                .trim()
                .parse()
                .with_context(|| format!("标点标签 id 非法：{id_text}"))?;
            if entries.len() <= id {
                entries.resize(id + 1, None);
            }
            if entries[id].replace(label.to_string()).is_some() {
                bail!("标点标签 id {id} 重复");
            }
        }
        if entries.is_empty() {
            bail!("标点标签表为空：{}", out_dict.display());
        }
        let labels: Vec<String> = entries
            .into_iter()
            .map(|label| label.unwrap_or_default())
            .collect();
        const EXPECTED: [&str; 5] = ["<space>", "，", "。", "？", "！"];
        if labels.len() != EXPECTED.len()
            || labels
                .iter()
                .zip(EXPECTED)
                .any(|(actual, expected)| actual != expected)
        {
            bail!(
                "标点标签必须为 <space> / ， / 。 / ？ / ！，实际 {:?}",
                labels
            );
        }
        let text = std::fs::read_to_string(punc_json)
            .with_context(|| format!("无法读取 {}", punc_json.display()))?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .with_context(|| format!("格式错误 {}", punc_json.display()))?;
        if let Some(list) = value.get("punc_list").and_then(|value| value.as_array()) {
            let list: Vec<&str> = list.iter().filter_map(|value| value.as_str()).collect();
            if list.len() != labels.len()
                || list
                    .iter()
                    .zip(&labels)
                    .any(|(actual, expected)| *actual != expected)
            {
                bail!("punc.json 标签 {:?} 与 out_dict {:?} 不一致", list, labels);
            }
        }
        Ok(Self { labels })
    }

    pub fn label(&self, id: usize) -> Option<&str> {
        self.labels.get(id).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.labels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }

    pub fn insert(&self, text: &str, pieces: &[Piece], ids: &[usize]) -> anyhow::Result<String> {
        let mut insertions: Vec<(usize, String)> = Vec::new();
        for (index, piece) in pieces.iter().enumerate() {
            if let Some(&label_id) = ids.get(index) {
                let Some(label) = self.label(label_id) else {
                    continue;
                };
                if label_id == 0 || label.trim().is_empty() || label == "<space>" {
                    continue;
                }
                if piece.end > 0 && piece.end <= text.len() {
                    insertions.push((piece.end, label.to_string()));
                }
            }
        }
        insertions.sort_by_key(|(offset, _)| *offset);
        let mut output = String::with_capacity(text.len() + insertions.len() * 3);
        let mut cursor = 0usize;
        for (offset, label) in insertions {
            if offset < cursor || offset > text.len() {
                continue;
            }
            output.push_str(&text[cursor..offset]);
            output.push_str(&label);
            cursor = offset;
        }
        output.push_str(&text[cursor..]);
        Ok(output)
    }
}

pub fn argmax(values: &[f32]) -> usize {
    let mut best = 0;
    for (index, value) in values.iter().enumerate() {
        if *value > values[best] {
            best = index;
        }
    }
    best
}
