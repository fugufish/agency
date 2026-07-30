pub mod claude;
pub mod codex;
pub mod native;

use agency_translator_api::{
    ClientId, ExportResult, ImportResult, NativeArtifact, SessionTranslator, TranslationError,
};

pub fn built_in(client: &ClientId) -> Option<Box<dyn SessionTranslator>> {
    match client.0.as_str() {
        "claude-code" => Some(Box::new(claude::ClaudeTranslator)),
        "codex" => Some(Box::new(codex::CodexTranslator)),
        _ => None,
    }
}

pub struct TranslationPipeline {
    source: Box<dyn SessionTranslator>,
    destination: Box<dyn SessionTranslator>,
}

impl TranslationPipeline {
    pub fn built_in(source: &ClientId, destination: &ClientId) -> Result<Self, TranslationError> {
        let source = built_in(source)
            .ok_or_else(|| TranslationError::new("no translator is registered for the source"))?;
        let destination = built_in(destination).ok_or_else(|| {
            TranslationError::new("no translator is registered for the destination")
        })?;
        Ok(Self {
            source,
            destination,
        })
    }

    pub fn translate(
        &self,
        artifact: &NativeArtifact,
    ) -> Result<(ImportResult, ExportResult), TranslationError> {
        self.source.validate(artifact)?;
        let imported = self.source.import(artifact)?;
        let exported = self.destination.export(&imported.conversation)?;
        self.destination.validate(&exported.artifact)?;
        Ok((imported, exported))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agency_translator_api::NativeArtifact;
    use serde_json::Value;

    #[test]
    fn translates_claude_jsonl_into_codex_history() {
        let pipeline =
            TranslationPipeline::built_in(&ClientId::new("claude-code"), &ClientId::new("codex"))
                .unwrap();
        let (_, exported) = pipeline
            .translate(&NativeArtifact::JsonLines(
                r#"{"type":"user","uuid":"one","message":{"role":"user","content":"Hello"}}
{"type":"assistant","uuid":"two","parentUuid":"one","message":{"role":"assistant","content":"Hi"}}"#
                    .to_owned(),
            ))
            .unwrap();
        let NativeArtifact::Json(Value::Array(items)) = exported.artifact else {
            panic!("expected Codex history items");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[1]["role"], "assistant");
    }
}
