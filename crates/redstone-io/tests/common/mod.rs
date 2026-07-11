use std::collections::BTreeMap;

use fastnbt::Value;
use redstone_core::BlockStateId;
use redstone_io::StructureStateResolver;

#[derive(Default)]
pub struct TestResolver {
    pub states: BTreeMap<String, BlockStateId>,
}

impl StructureStateResolver for TestResolver {
    type Error = std::io::Error;

    fn resolve_state(
        &mut self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<BlockStateId, Self::Error> {
        if name == "minecraft:air" {
            return Ok(self.air_state());
        }
        let key = format!("{name}{properties:?}");
        let next = BlockStateId(self.states.len() as u32 + 1);
        Ok(*self.states.entry(key).or_insert(next))
    }

    fn complete_state_properties(
        &mut self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, String>, Self::Error> {
        let mut completed = match name {
            "minecraft:repeater" => BTreeMap::from([
                ("delay".to_owned(), "1".to_owned()),
                ("facing".to_owned(), "north".to_owned()),
                ("locked".to_owned(), "false".to_owned()),
                ("powered".to_owned(), "false".to_owned()),
            ]),
            _ => BTreeMap::new(),
        };
        completed.extend(properties.clone());
        Ok(completed)
    }

    fn air_state(&self) -> BlockStateId {
        BlockStateId(0)
    }
}

#[allow(dead_code)]
pub fn block_state(name: &str, properties: &[(&str, &str)]) -> Value {
    let mut state = BTreeMap::from([("Name".to_owned(), Value::String(name.to_owned()))]);
    if !properties.is_empty() {
        state.insert(
            "Properties".to_owned(),
            Value::Compound(
                properties
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), Value::String((*value).to_owned())))
                    .collect(),
            ),
        );
    }
    Value::Compound(state.into_iter().collect())
}
