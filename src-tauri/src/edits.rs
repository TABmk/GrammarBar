use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    pub original: String,
    pub replacement: String,
    pub explanation: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Correction {
    pub segments: Vec<Segment>,
}

impl Correction {
    pub fn validate(&self, source: &str) -> Result<(), String> {
        if self.segments.len() > 2000 {
            return Err("Too many edits.".into());
        }
        let reconstructed: String = self.segments.iter().map(|s| s.original.as_str()).collect();
        if reconstructed != source {
            return Err("Could not match the edits. Please try again.".into());
        }
        for s in &self.segments {
            if s.original == s.replacement && !s.explanation.is_empty()
                || s.original != s.replacement && s.explanation.trim().is_empty()
                || s.original.is_empty() && s.replacement.is_empty()
                || s.explanation.len() > 4000
            {
                return Err("Invalid response. Please try again.".into());
            }
        }
        Ok(())
    }
}

pub fn schema() -> serde_json::Value {
    serde_json::json!({"type":"object","additionalProperties":false,"required":["segments"],"properties":{
        "segments":{"type":"array","items":{"type":"object","additionalProperties":false,
            "required":["original","replacement","explanation"],"properties":{
            "original":{"type":"string"},"replacement":{"type":"string"},"explanation":{"type":"string"}
        }}}
    }})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_lossless_unicode_edits_and_explanations() {
        let c: Correction = serde_json::from_value(serde_json::json!({"segments":[
            {"original":"🙂 I ","replacement":"🙂 I ","explanation":""},
            {"original":"has","replacement":"have","explanation":"Use have with I."},
            {"original":" tea\n","replacement":" tea\n","explanation":""},
            {"original":"","replacement":".","explanation":"Punctuation."}
        ]}))
        .unwrap();
        assert!(c.validate("🙂 I has tea\n").is_ok());
        assert!(c.validate("I has tea\n").is_err());
        let invalid = Correction {
            segments: vec![Segment {
                original: "a".into(),
                replacement: "b".into(),
                explanation: "".into(),
            }],
        };
        assert!(invalid.validate("a").is_err());
    }
}
