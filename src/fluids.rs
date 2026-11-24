//! Tinkers' fluids (TinkerFluids): the table the content generator writes (`gen_fluids.rs`) and what the client draws
//! the world fluids with.
//!
//! Every fluid is a real fork fluid (`data/tconstruct/sunrise/fluid/*.json`): 70 world fluids with their own liquid
//! block and bucket, the potion and powdered snow tank-only. Tanks, pipes and machines of every mod move them through
//! the FLUIDS capability, in mB.

use mod_api::*;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The parsed fluid table.
pub(crate) struct FluidTable {
    /// Fluid id to its still and flowing sprites and tint.
    pub textures: HashMap<String, (String, String, u32)>,
    /// Fluid id to its bucket item.
    pub buckets: HashMap<String, String>,
    /// Fluid tags (`c:molten_iron`...) to their fluids, resolved.
    pub tags: HashMap<String, Vec<String>>,
    /// The fluids Java gives a block (world fluids here).
    pub world: Vec<String>,
    /// Light levels (FluidType.lightLevel).
    pub light: HashMap<String, u8>,
    /// Hot fluids (FluidType temperature over 400: the melter's and the casting's "hot" checks).
    pub hot: Vec<String>,
}

/// FluidType.getLightLevel, 0 to 15: Tinkers' own from the fluid table, any other fluid (lava, another mod's) as the
/// game registered it (`fluid_type_properties`), read once per fluid and kept: the registry is fixed until restart.
pub(crate) fn light(host: &dyn Host, fluid: &str) -> u8 {
    if let Some(l) = table().light.get(fluid) {
        return (*l).min(15);
    }
    static OTHERS: std::sync::Mutex<Option<HashMap<String, u8>>> = std::sync::Mutex::new(None);
    let mut g = OTHERS.lock().unwrap_or_else(|e| e.into_inner());
    let known = g.get_or_insert_with(HashMap::new);
    if let Some(l) = known.get(fluid) {
        return *l;
    }
    let Some(props) = host.fluid_key(fluid).and_then(|k| host.fluid_states().type_properties(k)) else { return 0 };
    let l = props.light_level.min(15);
    known.insert(fluid.to_owned(), l);
    l
}

pub(crate) fn table() -> &'static FluidTable {
    static T: OnceLock<FluidTable> = OnceLock::new();
    T.get_or_init(|| {
        let v: serde_json::Value = serde_json::from_str(crate::gen_fluids::FLUIDS_JSON).unwrap_or_default();
        let strs = |v: &serde_json::Value| -> Vec<String> {
            v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
        };
        let mut t = FluidTable {
            textures: HashMap::new(),
            buckets: HashMap::new(),
            tags: HashMap::new(),
            world: strs(&v["world"]),
            light: HashMap::new(),
            hot: strs(&v["hot"]),
        };
        if let Some(o) = v["textures"].as_object() {
            for (k, x) in o {
                let s = |n: &str| x[n].as_str().unwrap_or_default().to_owned();
                t.textures.insert(k.clone(), (s("still"), s("flowing"), x["color"].as_u64().unwrap_or(0xFFFF_FFFF) as u32));
            }
        }
        if let Some(o) = v["buckets"].as_object() {
            for (k, x) in o {
                t.buckets.insert(k.clone(), x.as_str().unwrap_or_default().to_owned());
            }
        }
        if let Some(o) = v["tags"].as_object() {
            for (k, x) in o {
                t.tags.insert(k.clone(), strs(x));
            }
        }
        if let Some(o) = v["light"].as_object() {
            for (k, x) in o {
                t.light.insert(k.clone(), x.as_u64().unwrap_or(0) as u8);
            }
        }
        t
    })
}

/// The world fluids' flowing sprites (Java's `FluidTexture`: still, flowing; the overlay is the flowing one).
pub(crate) fn fluid_models(event: &mut RegisterFluidModelsEvent<'_>) {
    let t = table();
    for id in &t.world {
        if let Some((still, flowing, _)) = t.textures.get(id) {
            let mut m = FluidModel::default();
            m.fluid = id.clone();
            m.still = Some(still.clone());
            m.flowing = Some(flowing.clone());
            event.register(m);
        }
    }
}
