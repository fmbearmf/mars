use alloc::{sync::Arc, vec::Vec};

use crate::{
    IoError,
    device::{Device, Provider},
};

pub trait Layer: Send + Sync {
    fn create(&self, inputs: &[Arc<Provider>]) -> Result<Arc<dyn Device>, IoError>;
}

pub struct Graph {
    providers: Vec<Arc<Provider>>,
}

impl Graph {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    pub fn add_device(&mut self, device: Arc<dyn Device>) -> Result<Arc<Provider>, IoError> {
        let provider = Provider::new(device);

        provider.publish()?;

        self.providers.push(Arc::clone(&provider));

        Ok(provider)
    }

    pub fn stack(
        &mut self,
        layer: &dyn Layer,
        inputs: &[Arc<Provider>],
    ) -> Result<Arc<Provider>, IoError> {
        if inputs.is_empty() {
            return Err(IoError::InvalidTopology);
        }

        for input in inputs {
            let registered = self.providers.iter().any(|p| Arc::ptr_eq(p, input));

            if !registered {
                return Err(IoError::InvalidTopology);
            }
        }

        let device = layer.create(inputs)?;

        self.add_device(device)
    }

    pub fn providers(&self) -> &[Arc<Provider>] {
        &self.providers
    }
}

impl Default for Graph {
    fn default() -> Self {
        Self::new()
    }
}
