mod orientation;
mod registry;
mod rules;

pub use registry::{
    BlockBehavior, Java26Registry, PushReaction, StateDefinition, StateResolveError, StateResolver,
};
pub use rules::Java26Rules;

pub const JAVA_VERSION: &str = "26.1.2";
pub const JAVA_DATA_VERSION: i32 = 4790;
