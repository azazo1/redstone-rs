use redstone_core::BlockPos;
use smallvec::SmallVec;

pub(super) fn optimize_dependencies(dependencies: &mut SmallVec<[BlockPos; 16]>) {
    dependencies.sort_unstable();
    dependencies.dedup();
}

#[cfg(test)]
mod tests {
    use smallvec::smallvec;

    use super::*;

    #[test]
    fn duplicate_dependencies_are_removed_in_stable_order() {
        let mut dependencies = smallvec![
            BlockPos::new(2, 0, 0),
            BlockPos::new(1, 0, 0),
            BlockPos::new(2, 0, 0),
        ];

        optimize_dependencies(&mut dependencies);

        assert_eq!(
            dependencies.as_slice(),
            [BlockPos::new(1, 0, 0), BlockPos::new(2, 0, 0)]
        );
    }
}
