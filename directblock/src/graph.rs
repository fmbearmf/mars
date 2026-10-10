use alloc::{sync::Arc, vec, vec::Vec};

use crate::{
    IoError, IoResult, MAX_GRAPH_DEPTH,
    device::{Device, Provider, ProviderState},
};

struct Node {
    provider: Arc<Provider>,
    parents: Vec<usize>,
    depth: usize,
}

pub trait Layer: Send + Sync {
    fn create(&self, inputs: &[Arc<Provider>]) -> Result<Arc<dyn Device>, IoError>;
}

pub struct Graph {
    nodes: Vec<Node>,
}

impl Graph {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    pub fn add_device(&mut self, device: Arc<dyn Device>) -> Result<Arc<Provider>, IoError> {
        self.insert(device, Vec::new(), 1)
    }

    fn insert(
        &mut self,
        device: Arc<dyn Device>,
        parents: Vec<usize>,
        depth: usize,
    ) -> Result<Arc<Provider>, IoError> {
        if depth > MAX_GRAPH_DEPTH {
            return Err(IoError::InvalidTopology);
        }

        let provider = Provider::new(device);
        provider.publish()?;

        self.nodes.push(Node {
            provider: Arc::clone(&provider),
            parents,
            depth,
        });

        Ok(provider)
    }

    fn index(&self, provider: &Arc<Provider>) -> Option<usize> {
        self.nodes
            .iter()
            .position(|node| Arc::ptr_eq(&node.provider, provider))
    }

    pub fn stack(
        &mut self,
        layer: &dyn Layer,
        inputs: &[Arc<Provider>],
    ) -> Result<Arc<Provider>, IoError> {
        if inputs.is_empty() {
            return Err(IoError::InvalidTopology);
        }

        let mut parents = Vec::new();
        let mut max_depth = 0;

        for input in inputs {
            let index = self.index(input).ok_or(IoError::InvalidTopology)?;

            if input.state() != ProviderState::Online {
                return Err(IoError::NotReady);
            }

            if parents.contains(&index) {
                return Err(IoError::InvalidTopology);
            }

            max_depth = max_depth.max(self.nodes[index].depth);
            parents.push(index);
        }

        let depth = max_depth.checked_add(1).ok_or(IoError::InvalidTopology)?;

        if depth > MAX_GRAPH_DEPTH {
            return Err(IoError::InvalidTopology);
        }

        let device = layer.create(inputs)?;
        self.insert(device, parents, depth)
    }

    fn affected(&self, provider: &Arc<Provider>) -> Result<Vec<usize>, IoError> {
        let root = self.index(provider).ok_or(IoError::InvalidTopology)?;

        let mut selected = vec![false; self.nodes.len()];
        selected[root] = true;

        for i in (root + 1)..self.nodes.len() {
            selected[i] = self.nodes[i].parents.iter().any(|&parent| selected[parent]);
        }

        Ok(selected
            .iter()
            .enumerate()
            .filter_map(|(i, chosen)| chosen.then_some(i))
            .collect())
    }

    /// stop new I/O at all dependent providers before the parent
    pub fn quiesce(&self, provider: &Arc<Provider>) -> IoResult {
        let affected = self.affected(provider)?;

        for i in affected.into_iter().rev() {
            let p = &self.nodes[i].provider;

            if p.state() == ProviderState::Online {
                p.quiesce()?;
            }
        }

        Ok(())
    }

    /// propagate failure
    pub fn fail(&self, provider: &Arc<Provider>) -> IoResult {
        let affected = self.affected(provider)?;

        for i in affected.into_iter().rev() {
            self.nodes[i].provider.fail()?;
        }

        Ok(())
    }

    pub fn try_offline(&self, provider: &Arc<Provider>) -> IoResult {
        let affected = self.affected(provider)?;

        for i in affected.into_iter().rev() {
            let p = &self.nodes[i].provider;

            if p.state() != ProviderState::Offline {
                p.try_offline()?;
            }
        }

        Ok(())
    }

    pub fn providers(&self) -> impl Iterator<Item = &Arc<Provider>> {
        self.nodes.iter().map(|node| &node.provider)
    }
}

impl Default for Graph {
    fn default() -> Self {
        Self::new()
    }
}
