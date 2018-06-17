use std::cmp::{max, min};
use std::usize;

use engine::*;
use engine::aggregator::Aggregator;
use engine::types::*;
use engine::vector_op::comparator::*;
use errors::QueryError;


pub struct BatchResult<'a> {
    pub group_by: Option<Vec<BoxedVec<'a>>>,
    pub sort_by: Option<usize>,
    pub desc: bool,
    pub select: Vec<BoxedVec<'a>>,
    pub aggregators: Vec<Aggregator>,
    pub level: u32,
    pub batch_count: usize,
}

impl<'a> BatchResult<'a> {
    pub fn len(&self) -> usize {
        match self.group_by {
            Some(ref g) => g[0].len(),
            None => self.select.get(0).map_or(0, |s| s.len()),
        }
    }

    pub fn validate(&self) -> Result<(), QueryError> {
        let mut lengths = Vec::new();
        let mut info_str = "".to_owned();
        if let Some(ref group_bys) = self.group_by {
            for (i, group_by) in group_bys.iter().enumerate() {
                lengths.push(group_by.len());
                info_str = format!("{}:group_by[{}].len = {}", info_str, i, group_by.len());
            }
        }
        for (i, select) in self.select.iter().enumerate() {
            lengths.push(select.len());
            info_str = format!("{}:select[{}].len = {}", info_str, i, select.len()).to_owned();
        }
        let all_lengths_same = lengths.iter().all(|x| *x == lengths[0]);
        if all_lengths_same {
            Ok(())
        } else {
            Err(QueryError::FatalError(info_str))
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum MergeOp {
    TakeLeft,
    TakeRight,
    MergeRight,
}

#[derive(Debug, PartialEq)]
// TODO(clemens): u16 will not always be large enough
struct Premerge {
    left: u32,
    right: u32,
}

pub fn combine<'a>(batch1: BatchResult<'a>, batch2: BatchResult<'a>, limit: usize) -> Result<BatchResult<'a>, QueryError> {
    match (batch1.group_by, batch2.group_by) {
        // Aggregation query
        (Some(g1), Some(g2)) => {
            let (group_by_cols, ops) = if g1.len() == 1 {
                // TODO(clemens): other types, val coercion
                let (merged_grouping, ops) = match (g1[0].get_type(), g2[0].get_type()) {
                    (EncodingType::Str, EncodingType::Str) =>
                        merge_deduplicate(g1[0].cast_ref_str(), g2[0].cast_ref_str()),
                    (EncodingType::U8, EncodingType::U8) =>
                        merge_deduplicate(g1[0].cast_ref_u8(), g2[0].cast_ref_u8()),
                    (EncodingType::I64, EncodingType::I64) =>
                        merge_deduplicate(g1[0].cast_ref_i64(), g2[0].cast_ref_i64()),
                    (t1, t2) => bail!(QueryError::NotImplemented, "merge_deduplicate types {:?}, {:?}", t1, t2),
                };
                (vec![merged_grouping], ops)
            } else {
                let mut partitioning = match (g1[0].get_type(), g2[0].get_type()) {
                    (EncodingType::Str, EncodingType::Str) =>
                        partition::<&str>(g1[0].as_ref(), g2[0].as_ref(), usize::MAX),
                    (EncodingType::I64, EncodingType::I64) =>
                        partition::<i64>(g1[0].as_ref(), g2[0].as_ref(), usize::MAX),
                    (t1, t2) => bail!(QueryError::NotImplemented, "partition types {:?}, {:?}", t1, t2),
                };

                for i in 1..(g1.len() - 1) {
                    partitioning = match (g1[i].get_type(), g2[i].get_type()) {
                        (EncodingType::Str, EncodingType::Str) =>
                            subpartition::<&str>(&partitioning, g1[i].as_ref(), g2[i].as_ref()),
                        (EncodingType::I64, EncodingType::I64) =>
                            subpartition::<i64>(&partitioning, g1[i].as_ref(), g2[i].as_ref()),
                        (t1, t2) => bail!(QueryError::NotImplemented, "partition types {:?}, {:?}", t1, t2),
                    };
                }

                let last = g1.len() - 1;
                let (merged_grouping, ops) = match (g1[last].get_type(), g2[last].get_type()) {
                    (EncodingType::Str, EncodingType::Str) =>
                        merge_deduplicate_partitioned::<&'a str>(&partitioning, g1[last].as_ref(), g2[last].as_ref()),
                    (EncodingType::I64, EncodingType::I64) =>
                        merge_deduplicate_partitioned::<i64>(&partitioning, g1[last].as_ref(), g2[last].as_ref()),
                    (t1, t2) => bail!(QueryError::NotImplemented, "merge_deduplicate_partitioned types {:?}, {:?}", t1, t2),
                };

                let mut group_by_cols = Vec::with_capacity(g1.len());
                for i in 0..last {
                    group_by_cols.push(match (g1[i].get_type(), g2[i].get_type()) {
                        (EncodingType::Str, EncodingType::Str) =>
                            merge_drop::<&str>(g1[i].as_ref(), g2[i].as_ref(), &ops),
                        (EncodingType::I64, EncodingType::I64) =>
                            merge_drop::<i64>(g1[i].as_ref(), g2[i].as_ref(), &ops),
                        (t1, t2) => bail!(QueryError::NotImplemented, "merge_drop types {:?}, {:?}", t1, t2),
                    });
                }
                group_by_cols.push(merged_grouping);

                (group_by_cols, ops)
            };

            let mut aggregates = Vec::with_capacity(batch1.aggregators.len());
            for (i, aggregator) in batch1.aggregators.iter().enumerate() {
                let merged = merge_aggregate(
                    batch1.select[i].cast_ref_i64(),
                    batch2.select[i].cast_ref_i64(),
                    &ops, *aggregator);
                aggregates.push(merged);
            }
            Ok(BatchResult {
                group_by: Some(group_by_cols),
                sort_by: None,
                desc: batch1.desc,
                select: aggregates,
                aggregators: batch1.aggregators,
                level: batch1.level + 1,
                batch_count: batch1.batch_count + batch2.batch_count,
            })
        }
        // No aggregation
        (None, None) => {
            match batch1.sort_by {
                // Sort query
                Some(index) => {
                    let (merged_sort_col, ops) = {
                        let s1 = &batch1.select[index];
                        let s2 = &batch2.select[index];
                        match (s1.get_type(), s2.get_type()) {
                            (EncodingType::Str, EncodingType::Str) => if batch1.desc {
                                merge_sort::<_, CmpGreaterThan>(s1.cast_ref_str(), s2.cast_ref_str(), limit)
                            } else {
                                merge_sort::<_, CmpLessThan>(s1.cast_ref_str(), s2.cast_ref_str(), limit)
                            },
                            (EncodingType::I64, EncodingType::I64) => if batch1.desc {
                                merge_sort::<_, CmpGreaterThan>(s1.cast_ref_i64(), s2.cast_ref_i64(), limit)
                            } else {
                                merge_sort::<_, CmpLessThan>(s1.cast_ref_i64(), s2.cast_ref_i64(), limit)
                            },
                            (t1, t2) => bail!(QueryError::NotImplemented, "merge_sort types {:?}, {:?}", t1, t2),
                        }
                    };

                    let mut result = Vec::with_capacity(batch1.select.len());
                    for (i, (col1, col2)) in batch1.select.into_iter().zip(batch2.select).enumerate() {
                        if i == index {
                            result.push(TypedVec::empty(0));
                        } else {
                            let merged = match (col1.get_type(), col2.get_type()) {
                                (EncodingType::Str, EncodingType::Str) =>
                                    merge(col1.cast_ref_str(), col2.cast_ref_str(), &ops),
                                (EncodingType::I64, EncodingType::I64) =>
                                    merge(col1.cast_ref_i64(), col2.cast_ref_i64(), &ops),
                                (t1, t2) => bail!(QueryError::NotImplemented, "merge types {:?}, {:?}", t1, t2),
                            };
                            result.push(merged);
                        }
                    }
                    result[index] = merged_sort_col;

                    Ok(BatchResult {
                        group_by: None,
                        sort_by: Some(index),
                        select: result,
                        desc: batch1.desc,
                        aggregators: Vec::new(),
                        level: batch1.level + 1,
                        batch_count: batch1.batch_count + batch2.batch_count,
                    })
                }
                // Select query
                None => {
                    let mut result = Vec::with_capacity(batch1.select.len());
                    for (mut col1, col2) in batch1.select.into_iter().zip(batch2.select) {
                        let count = if col1.len() >= limit { 0 } else {
                            min(col2.len(), limit - col1.len())
                        };
                        if let Some(newcol) = col1.extend(col2, count) {
                            result.push(newcol)
                        } else {
                            result.push(col1)
                        }
                    }
                    Ok(BatchResult {
                        group_by: None,
                        sort_by: None,
                        select: result,
                        desc: batch1.desc,
                        aggregators: Vec::new(),
                        level: batch1.level + 1,
                        batch_count: batch1.batch_count + batch2.batch_count,
                    })
                }
            }
        }
        _ => bail!(QueryError::FatalError,  "Trying to merge incompatible batch results")
    }
}

fn merge_deduplicate<'a, T: VecType<T> + 'a>(left: &[T], right: &[T]) -> (BoxedVec<'a>, Vec<MergeOp>) {
    // TODO(clemens): figure out maths for precise estimate + variance derived from how much grouping reduced cardinality
    let output_len_estimate = max(left.len(), right.len()) + min(left.len(), right.len()) / 2;
    let mut result = Vec::with_capacity(output_len_estimate);
    let mut ops = Vec::<MergeOp>::with_capacity(output_len_estimate);

    let mut i = 0;
    let mut j = 0;
    while i < left.len() && j < right.len() {
        if result.last() == Some(&right[j]) {
            ops.push(MergeOp::MergeRight);
            j += 1;
        } else if left[i] <= right[j] {
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

    (TypedVec::owned(result), ops)
}

fn merge_deduplicate_partitioned<'a, T: VecType<T> + 'a>(partitioning: &[Premerge],
                                                         left: &TypedVec<'a>,
                                                         right: &TypedVec<'a>) -> (BoxedVec<'a>, Vec<MergeOp>) {
    let left = T::unwrap(left);
    let right = T::unwrap(right);
    let output_len_estimate = max(left.len(), right.len()) + min(left.len(), right.len()) / 2;
    let mut result = Vec::with_capacity(output_len_estimate);
    let mut ops = Vec::<MergeOp>::with_capacity(output_len_estimate);

    let mut i = 0;
    let mut j = 0;
    for group in partitioning {
        let mut last = None;
        let i_max = i + group.left as usize;
        let j_max = j + group.right as usize;
        // println!("i_max = {}, j_max = {}", i_max, j_max);
        for _ in 0..(group.left + group.right) {
            // println!("i = {}, j = {}, last = {:?}", i, j, last);
            // println!("{:?} {:?}", left.get(i), right.get(j));
            if j < j_max && last == Some(right[j]) {
                ops.push(MergeOp::MergeRight);
                j += 1;
            } else if i < i_max && (j >= j_max || left[i] <= right[j]) {
                result.push(left[i]);
                ops.push(MergeOp::TakeLeft);
                last = Some(left[i]);
                i += 1;
            } else {
                result.push(right[j]);
                ops.push(MergeOp::TakeRight);
                last = Some(right[j]);
                j += 1;
            }
            // println!("{:?}", ops.last().unwrap());
        }
    }
    (TypedVec::owned(result), ops)
}

fn partition<'a, T: VecType<T> + 'a>(left: &TypedVec<'a>, right: &TypedVec<'a>, limit: usize) -> Vec<Premerge> {
    let mut result = Vec::new();
    let left = T::unwrap(left);
    let right = T::unwrap(right);
    let mut i = 0;
    let mut j = 0;
    while i < left.len() && j < right.len() && i + j < limit {
        let mut partition = Premerge { left: 0, right: 0 };
        let elem = if left[i] <= right[j] { left[i] } else { right[j] };
        while i < left.len() && elem == left[i] {
            partition.left += 1;
            i += 1;
        }
        while j < right.len() && elem == right[j] {
            partition.right += 1;
            j += 1;
        }
        result.push(partition);
    }

    // Remaining elements on left
    while i < left.len() && i + j < limit {
        let elem = left[i];
        let i_start = i;
        while i < left.len() && elem == left[i] {
            i += 1;
        }
        result.push(Premerge { left: (i - i_start) as u32, right: 0 });
    }

    // Remaining elements on right
    while j < right.len() && i + j < limit {
        let elem = right[j];
        let j_start = j;
        while j < right.len() && elem == right[j] {
            j += 1;
        }
        result.push(Premerge { right: (j - j_start) as u32, left: 0 });
    }
    result
}

fn subpartition<'a, T: VecType<T> + 'a>(
    partitioning: &[Premerge],
    left: &TypedVec<'a>,
    right: &TypedVec<'a>) -> Vec<Premerge> {
    let left = T::unwrap(left);
    let right = T::unwrap(right);
    // TODO(clemens): better estimate?
    let mut result = Vec::with_capacity(2 * partitioning.len());
    let mut i = 0;
    let mut j = 0;
    for group in partitioning {
        let i_max = i + group.left as usize;
        let j_max = j + group.right as usize;
        while i < i_max || j < j_max {
            let mut subpartition = Premerge { left: 0, right: 0 };
            let elem = if i < i_max && (j == j_max || left[i] <= right[j]) { left[i] } else { right[j] };
            while i < i_max && elem == left[i] {
                subpartition.left += 1;
                i += 1;
            }
            while j < j_max && elem == right[j] {
                subpartition.right += 1;
                j += 1;
            }
            result.push(subpartition);
        }
    }
    result
}

fn merge_sort<'a, T: VecType<T> + 'a, C: Comparator<T>>(left: &[T], right: &[T], limit: usize) -> (BoxedVec<'a>, Vec<bool>) {
    let mut result = Vec::with_capacity(left.len() + right.len());
    let mut ops = Vec::<bool>::with_capacity(left.len() + right.len());

    let mut i = 0;
    let mut j = 0;
    while i < left.len() && j < right.len() && i + j < limit {
        if C::cmp(left[i], right[j]) {
            result.push(left[i]);
            ops.push(true);
            i += 1;
        } else {
            result.push(right[j]);
            ops.push(false);
            j += 1;
        }
    }

    for x in left[i..min(left.len(), limit - j)].iter() {
        result.push(*x);
        ops.push(true);
    }
    for x in right[j..min(right.len(), limit - i)].iter() {
        result.push(*x);
        ops.push(false);
    }

    (TypedVec::owned(result), ops)
}

fn merge_aggregate<'a>(left: &[i64], right: &[i64], ops: &[MergeOp], aggregator: Aggregator) -> BoxedVec<'a> {
    let mut result = Vec::with_capacity(ops.len());
    let mut i = 0;
    let mut j = 0;
    for op in ops {
        match *op {
            MergeOp::TakeLeft => {
                if i == left.len() {
                    error!("{} {} {}", left.len(), right.len(), ops.len());
                }
                result.push(left[i]);
                i += 1;
            }
            MergeOp::TakeRight => {
                if j == right.len() {
                    error!("{} {} {}", left.len(), right.len(), ops.len());
                }
                result.push(right[j]);
                j += 1;
            }
            MergeOp::MergeRight => {
                // TODO(clemens): make inlining of aggregator operation possible
                let last = result.len() - 1;
                result[last] = aggregator.combine_i64(result[last], right[j]);
                j += 1;
            }
        }
    }
    TypedVec::owned(result)
}

fn merge<'a, T: 'a>(left: &[T], right: &[T], ops: &[bool]) -> BoxedVec<'a>
    where T: VecType<T> {
    let mut result = Vec::with_capacity(ops.len());
    let mut i = 0;
    let mut j = 0;
    for take_left in ops {
        if *take_left {
            result.push(left[i]);
            i += 1;
        } else {
            result.push(right[j]);
            j += 1;
        }
    }
    TypedVec::owned(result)
}

fn merge_drop<'a, T: VecType<T> + 'a>(left: &TypedVec<'a>, right: &TypedVec<'a>, ops: &[MergeOp]) -> BoxedVec<'a> {
    let left = T::unwrap(left);
    let right = T::unwrap(right);
    // TODO(clemens): this is an overestimate
    let mut result = Vec::with_capacity(ops.len());
    let mut i = 0;
    let mut j = 0;
    for op in ops {
        match *op {
            MergeOp::TakeLeft => {
                result.push(left[i]);
                i += 1;
            }
            MergeOp::TakeRight => {
                result.push(right[j]);
                j += 1;
            }
            MergeOp::MergeRight => {
                j += 1;
            }
        }
    }
    TypedVec::owned(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multipass_grouping() {
        let left1 = vec!["A", "A", "A", "C", "P"];
        let right1 = vec!["A", "A", "B", "C", "X", "X", "Z"];
        let result = partition::<&str>(&left1, &right1, 10);
        assert_eq!(result, vec![
            Premerge { left: 3, right: 2 },
            Premerge { left: 0, right: 1 },
            Premerge { left: 1, right: 1 },
            Premerge { left: 1, right: 0 },
            Premerge { left: 0, right: 2 },
        ]);

        let left2 = vec![1u32, 3, 7, 2, 1];
        let right2 = vec![3u32, 5, 0, 2, 1, 2, 1];
        let (merging, merge_ops) = merge_deduplicate_partitioned::<u32>(&result, &left2, &right2);
        assert_eq!(u32::unwrap(merging.as_ref()), &[1, 3, 5, 7, 0, 2, 1, 1, 2]);
        use self::MergeOp::*;
        assert_eq!(&merge_ops, &[
            TakeLeft,
            TakeLeft,
            MergeRight,
            TakeRight,
            TakeLeft,
            TakeRight,
            TakeLeft,
            MergeRight,
            TakeLeft,
            TakeRight,
            TakeRight,
        ]);

        let subpartition = subpartition::<u32>(&result, &left2, &right2);
        assert_eq!(subpartition, vec![
            Premerge { left: 1, right: 0 },
            Premerge { left: 1, right: 1 },
            Premerge { left: 0, right: 1 },
            Premerge { left: 1, right: 0 },
            Premerge { left: 0, right: 1 },
            Premerge { left: 1, right: 1 },
            Premerge { left: 1, right: 0 },
            Premerge { left: 0, right: 1 },
            Premerge { left: 0, right: 1 },
        ]);
    }
}