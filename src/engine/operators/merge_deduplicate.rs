use crate::engine::*;
use std::cmp::{max, min};
use std::marker::PhantomData;

#[derive(Debug)]
pub struct MergeDeduplicate<T, C> {
    pub left: BufferRef<T>,
    pub right: BufferRef<T>,
    pub deduplicated: BufferRef<T>,
    pub merge_ops: BufferRef<MergeOp>,
    pub comparator: PhantomData<C>,
}

impl<'a, T: VecData<T> + 'a, C: Comparator<T>> VecOperator<'a> for MergeDeduplicate<T, C> {
    fn execute(&mut self, _: bool, scratchpad: &mut Scratchpad<'a>) -> Result<(), QueryError> {
        let (deduplicated, merge_ops) = {
            let left = scratchpad.get(self.left);
            let right = scratchpad.get(self.right);
            merge_deduplicate::<_, C>(&left, &right)
        };
        scratchpad.set(self.deduplicated, deduplicated);
        scratchpad.set(self.merge_ops, merge_ops);
        Ok(())
    }

    fn inputs(&self) -> Vec<BufferRef<Any>> { vec![self.left.any(), self.right.any()] }
    fn inputs_mut(&mut self) -> Vec<&mut usize> { vec![&mut self.left.i, &mut self.right.i] }
    fn outputs(&self) -> Vec<BufferRef<Any>> { vec![self.deduplicated.any(), self.merge_ops.any()] }
    fn can_stream_input(&self, _: usize) -> bool { false }
    fn can_stream_output(&self, _: usize) -> bool { false }
    fn allocates(&self) -> bool { true }

    fn display_op(&self, _: bool) -> String {
        format!("merge_deduplicate({}, {})", self.left, self.right)
    }
}

fn merge_deduplicate<'a, T: VecData<T> + 'a, C: Comparator<T>>(left: &[T], right: &[T]) -> (Vec<T>, Vec<MergeOp>) {
    // Could figure out maths for more precise estimate + variance derived from how much grouping reduced cardinality
    let output_len_estimate = max(left.len(), right.len()) + min(left.len(), right.len()) / 2;
    let mut result = Vec::with_capacity(output_len_estimate);
    let mut ops = Vec::<MergeOp>::with_capacity(output_len_estimate);

    let mut i = 0;
    let mut j = 0;
    while i < left.len() && j < right.len() {
        if result.last() == Some(&right[j]) {
            ops.push(MergeOp::MergeRight);
            j += 1;
        } else if C::cmp_eq(left[i], right[j]) {
            result.push(left[i]);
            ops.push(MergeOp::TakeLeft);
            i += 1;
        } else {
            result.push(right[j]);
            ops.push(MergeOp::TakeRight);
            j += 1;
        }
    }

    for x in left[i..].iter() {
        result.push(*x);
        ops.push(MergeOp::TakeLeft);
    }
    if j < right.len() && result.last() == Some(&right[j]) {
        ops.push(MergeOp::MergeRight);
        j += 1;
    }
    for x in right[j..].iter() {
        result.push(*x);
        ops.push(MergeOp::TakeRight);
    }

    (result, ops)
}


