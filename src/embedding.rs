use anyhow::{Context, Result};
use model2vec_rs::model::StaticModel;
use std::path::Path;

pub struct MinishEmbeddingModel {
    model: StaticModel,
}

impl MinishEmbeddingModel {
    pub fn load(model_id: &str) -> Result<Self> {
        let model = StaticModel::from_pretrained(model_id, None, None, None)
            .with_context(|| format!("failed to load Model2Vec model: {model_id}"))?;
        Ok(Self { model })
    }

    pub fn load_local(path: &Path) -> Result<Self> {
        let display = path.display().to_string();
        let model = StaticModel::from_pretrained(path, None, None, None)
            .with_context(|| format!("failed to load local Model2Vec model: {display}"))?;
        Ok(Self { model })
    }

    pub fn encode(&self, texts: &[String]) -> Vec<Vec<f32>> {
        self.model.encode(texts)
    }

    pub fn encode_one(&self, text: &str) -> Vec<f32> {
        self.model.encode_single(text)
    }
}
