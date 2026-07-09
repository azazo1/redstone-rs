mod format;
mod structure;
mod vector;

pub use structure::{StructureBlock, StructureError, StructureInput};
pub use vector::{diff_snapshots, SnapshotDifference, SimulationTrace, TestVector, TimedAction, VectorError};
