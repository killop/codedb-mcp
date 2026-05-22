use anyhow::{Context, Result};
use model2vec_rs::model::StaticModel;
use std::path::Path;

pub struct MinishEmbeddingModel {
    model_id: String,
    model: StaticModel,
    dim: usize,
}

impl MinishEmbeddingModel {
    pub fn load(model_id: &str) -> Result<Self> {
        let model = StaticModel::from_pretrained(model_id, None, None, None)
            .with_context(|| format!("failed to load Model2Vec model: {model_id}"))?;
        let dim = model.encode_single("dimension probe").len();
        Ok(Self {
            model_id: model_id.to_string(),
            model,
            dim,
        })
    }

    pub fn load_local(path: &Path) -> Result<Self> {
        let display = path.display().to_string();
        let model = StaticModel::from_pretrained(path, None, None, None)
            .with_context(|| format!("failed to load local Model2Vec model: {display}"))?;
        let dim = model.encode_single("dimension probe").len();
        Ok(Self {
            model_id: display,
            model,
            dim,
        })
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn encode(&self, texts: &[String]) -> Vec<Vec<f32>> {
        self.model.encode(texts)
    }

    pub fn encode_one(&self, text: &str) -> Vec<f32> {
        self.model.encode_single(text)
    }
}
