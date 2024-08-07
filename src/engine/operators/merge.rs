use crate::engine::*;
use std::cmp;
use std::fmt::Debug;
use std::marker::PhantomData;

#[derive(Debug)]
pub struct Merge<T, C: Debug> {
    pub left: BufferRef<T>,
    pub right: BufferRef<T>,
    pub merged: BufferRef<T>,
    pub merge_ops: BufferRef<u8>,
    pub limit: usize,
    pub c: PhantomData<C>,
}

impl<'a, T: VecData<T> + 'a, C: Comparator<T> + Debug> VecOperator<'a> for Merge<T, C> {
    fn execute(&mut self, _: bool, scratchpad: &mut Scratchpad<'a>) -> Result<(), QueryError> {
        let (merged, ops) = {
            let left = scratchpad.get(self.left);
            let right = scratchpad.get(self.right);
            merge::<_, C>(&left, &right, self.limit)
        };
        scratchpad.set(self.merged, merged);
        scratchpad.set(self.merge_ops, ops);
        Ok(())
    }

    fn inputs(&self) -> Vec<BufferRef<Any>> {
        vec![self.left.any(), self.right.any()]
    }
    fn inputs_mut(&mut self) -> Vec<&mut usize> {
        vec![&mut self.left.i, &mut self.right.i]
    }
    fn outputs(&self) -> Vec<BufferRef<Any>> {
        vec![self.merged.any(), self.merge_ops.any()]
    }
    fn can_stream_input(&self, _: usize) -> bool {
        false
    }
    fn can_stream_output(&self, _: usize) -> bool {
        false
    }
    fn allocates(&self) -> bool {
        true
    }

    fn display_op(&self, _: bool) -> String {
        format!("merge({}, {})", self.left, self.right)
    }
}

fn merge<'a, T: VecData<T> + 'a, C: Comparator<T>>(
    left: &[T],
    right: &[T],
    limit: usize,
) -> (Vec<T>, Vec<u8>) {
    let len = cmp::min(left.len() + right.len(), limit);
    let mut result = Vec::with_capacity(len);
    let mut ops = Vec::<u8>::with_capacity(len);

    let mut i = 0;
    let mut j = 0;
    while i < left.len() && j < right.len() && i + j < limit {
        if C::cmp_eq(left[i], right[j]) {
            result.push(left[i]);
            ops.push(1);
            i += 1;
        } else {
            result.push(right[j]);
            ops.push(0);
            j += 1;
        }
    }

    for x in left[i..cmp::min(left.len(), limit - j)].iter() {
        result.push(*x);
        ops.push(1);
    }
    for x in right[j..cmp::min(right.len(), limit - i)].iter() {
        result.push(*x);
        ops.push(0);
    }

    (result, ops)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::mem_store::Val;

    #[test]
    fn test_merge() {
        let left = vec![
            Val::Null,
            Val::Null,
            Val::Null,
            Val::Null,
            Val::Null,
            Val::from(0.4),
        ];
        let right = vec![
            Val::Null,
            Val::Null,
            Val::from(1.123124e30),
            Val::from(1e-32),
        ];
        let (merged, merge_ops) = merge::<Val, CmpGreaterThan>(&left, &right, 10);
        assert_eq!(
            merged,
            vec![
                Val::Null,
                Val::Null,
                Val::Null,
                Val::Null,
                Val::Null,
                Val::Null,
                Val::Null,
                Val::from(1.123124e30),
                Val::from(0.4),
                Val::from(1e-32),
            ]
        );
        assert_eq!(merge_ops, vec![1, 1, 1, 1, 1, 0, 0, 0, 1, 0]);
    }
}
