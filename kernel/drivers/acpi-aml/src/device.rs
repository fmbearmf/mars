use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use hashbrown::HashMap;
use klib::hardware::{
    device::{DeviceClass, DeviceId, DeviceInitPriority, DeviceTree},
    resource::Resource,
};

use crate::{
    ast::{AmlTerm, AmlValue, AmlValueExt},
    crs::{CrsIter, DecodeCrs},
    parser::AmlParser,
};

#[derive(Default)]
pub struct DeviceProperties<'a> {
    hid: Option<String>,
    cids: Vec<String>,
    uid: Option<u64>,
    resources: Vec<Resource>,
    sub_terms: Vec<AmlTerm<'a>>,
}

impl<'a> DeviceProperties<'a> {
    fn from_aml(parser: &mut AmlParser<'a>) -> Result<Self, &'static str> {
        let terms = core::iter::from_fn(|| parser.parse_next().transpose());

        terms.fold(Ok(Self::default()), |acc, term_res| {
            let mut acc = acc?;
            match term_res? {
                AmlTerm::Name { name, value } => {
                    let name_str = name.to_string();
                    if name_str.ends_with("_HID") {
                        acc.hid = value.as_string();
                    } else if name_str.ends_with("_CID") {
                        match &value {
                            AmlValue::Package(pkgs) => {
                                for p in pkgs {
                                    if let Some(s) = p.as_string() {
                                        acc.cids.push(s);
                                    }
                                }
                            }
                            _ => {
                                if let Some(s) = value.as_string() {
                                    acc.cids.push(s);
                                }
                            }
                        }
                    } else if name_str.ends_with("_UID") {
                        acc.uid = value.as_u64();
                    } else if name_str.ends_with("_CRS") {
                        if let AmlValue::Buffer(buf) = &value {
                            acc.resources
                                .extend(CrsIter(*buf).flat_map(|chunk| chunk.into_rss()));
                        }
                    } else {
                        acc.sub_terms.push(AmlTerm::Name { name, value });
                    }
                }
                other => acc.sub_terms.push(other),
            }
            Ok(acc)
        })
    }

    fn classify(&self) -> (DeviceClass, DeviceInitPriority) {
        let id = self
            .hid
            .as_deref()
            .or_else(|| self.cids.first().map(String::as_str))
            .unwrap_or("");
        match id {
            "ARMH0011" => (DeviceClass::Uart, DeviceInitPriority::Regular),
            "PNP0A08" | "PNP0A03" => (DeviceClass::PciHostBridge, DeviceInitPriority::Regular),
            _ => (DeviceClass::Other, DeviceInitPriority::Regular),
        }
    }
}

pub struct TreeBuilder<'a> {
    tree: &'a mut DeviceTree,
    scope_map: HashMap<String, DeviceId>,
}

impl<'a> TreeBuilder<'a> {
    pub fn new(tree: &'a mut DeviceTree) -> Self {
        Self {
            tree,
            scope_map: HashMap::new(),
        }
    }

    pub fn join_path(parent: &str, child: &str) -> String {
        if child.starts_with('\\') {
            child.to_string()
        } else if parent.is_empty() || parent == "\\" {
            format!("\\{}", child.trim_start_matches('.'))
        } else {
            format!("{}.{}", parent, child.trim_start_matches('.'))
        }
    }

    pub fn process_terms<'b>(
        &mut self,
        terms: impl IntoIterator<Item = AmlTerm<'b>>,
        current_path: &str,
        parent_id: Option<DeviceId>,
    ) -> Result<(), &'static str> {
        for term in terms {
            match term {
                AmlTerm::Scope { name, mut contents } => {
                    let path = Self::join_path(current_path, &name.to_string());
                    let target_parent = self.scope_map.get(&path).copied().or(parent_id);
                    let sub_terms = core::iter::from_fn(|| contents.parse_next().transpose())
                        .collect::<Result<Vec<_>, _>>()?;
                    self.process_terms(sub_terms, &path, target_parent)?;
                }
                AmlTerm::Device { name, mut contents } => {
                    let path = Self::join_path(current_path, &name.to_string());
                    let props = DeviceProperties::from_aml(&mut contents)?;

                    let (class, priority) = props.classify();
                    let compatible = props.hid.into_iter().chain(props.cids).collect();

                    let dev_id = self.tree.add_device(
                        parent_id,
                        class,
                        compatible,
                        props.resources,
                        priority,
                    );

                    self.scope_map.insert(path.clone(), dev_id);
                    self.process_terms(props.sub_terms, &path, Some(dev_id))?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}
