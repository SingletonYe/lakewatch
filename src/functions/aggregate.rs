//! The aggregate functions (UDAFs).
//!
//! `mode` is the interesting one: it keeps a value histogram per group and
//! implements the full partial/final protocol, so it also works when DataFusion
//! decides to split the aggregation across partitions. The internal state is a
//! single `Utf8` payload holding the histogram as JSON — one scalar per group,
//! which is exactly what `Accumulator::state` is allowed to return and keeps the
//! state schema trivially mergeable.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, StringArray, UInt64Array};
use datafusion::arrow::datatypes::{DataType, Field};
use datafusion::common::{DataFusionError, Result as DfResult};
use datafusion::logical_expr::function::{AccumulatorArgs, StateFieldsArgs};
use datafusion::logical_expr::{
    Accumulator, AggregateUDF, AggregateUDFImpl, Signature, Volatility,
};
use datafusion::scalar::ScalarValue;

// ---------------------------------------------------------------------------
// mode
// ---------------------------------------------------------------------------

/// `mode(expr) -> utf8`
pub fn mode_udaf() -> AggregateUDF {
    AggregateUDF::new_from_impl(ModeUdaf::new())
}

/// Aggregate-function definition for `mode`.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct ModeUdaf {
    signature: Signature,
}

impl ModeUdaf {
    /// New definition.
    pub fn new() -> Self {
        Self {
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

impl Default for ModeUdaf {
    fn default() -> Self {
        Self::new()
    }
}

impl AggregateUDFImpl for ModeUdaf {
    fn name(&self) -> &str {
        "mode"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> DfResult<DataType> {
        Ok(DataType::Utf8)
    }

    fn accumulator(&self, _args: AccumulatorArgs) -> DfResult<Box<dyn Accumulator>> {
        Ok(Box::new(ModeAccumulator::default()))
    }

    fn state_fields(&self, _args: StateFieldsArgs) -> DfResult<Vec<Arc<Field>>> {
        Ok(vec![Arc::new(Field::new(
            "histogram",
            DataType::Utf8,
            false,
        ))])
    }
}

/// Per-group value histogram.
#[derive(Debug, Default)]
struct ModeAccumulator {
    /// Rendered value -> occurrences.
    histogram: HashMap<String, u64>,
}

impl ModeAccumulator {
    fn merge(&mut self, other: HashMap<String, u64>) {
        for (key, count) in other {
            *self.histogram.entry(key).or_insert(0) += count;
        }
    }
}

impl Accumulator for ModeAccumulator {
    fn update_batch(&mut self, values: &[ArrayRef]) -> DfResult<()> {
        let array = values
            .first()
            .ok_or_else(|| DataFusionError::Execution("mode: expected one argument".to_string()))?;
        for index in 0..array.len() {
            let scalar = ScalarValue::try_from_array(array, index)?;
            if scalar.is_null() {
                continue;
            }
            *self.histogram.entry(render(&scalar)).or_insert(0) += 1;
        }
        Ok(())
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> DfResult<()> {
        let payload = states.first().ok_or_else(|| {
            DataFusionError::Execution("mode: expected one state column".to_string())
        })?;
        let payload = payload
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| {
                DataFusionError::Execution("mode: state column must be Utf8".to_string())
            })?;
        for index in 0..payload.len() {
            if payload.is_null(index) {
                continue;
            }
            let parsed: BTreeMap<String, u64> = serde_json::from_str(payload.value(index))
                .map_err(|e| {
                    DataFusionError::Execution(format!("mode: corrupt partial state: {e}"))
                })?;
            self.merge(parsed.into_iter().collect());
        }
        Ok(())
    }

    fn state(&mut self) -> DfResult<Vec<ScalarValue>> {
        let encoded = serde_json::to_string(&self.histogram).map_err(|e| {
            DataFusionError::Execution(format!("mode: cannot serialise state: {e}"))
        })?;
        Ok(vec![ScalarValue::Utf8(Some(encoded))])
    }

    fn evaluate(&mut self) -> DfResult<ScalarValue> {
        // Highest count wins; ties break on the smallest rendered value so the
        // output is deterministic across runs and across partition layouts.
        let winner = self
            .histogram
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(value, _)| value.clone());
        Ok(ScalarValue::Utf8(winner))
    }

    fn size(&self) -> usize {
        std::mem::size_of_val(self)
            + self
                .histogram
                .keys()
                .map(|key| key.capacity() + 8 + 48)
                .sum::<usize>()
    }
}

/// Canonical text form of a value, used as the histogram key.
fn render(scalar: &ScalarValue) -> String {
    match scalar {
        ScalarValue::Utf8(Some(v)) | ScalarValue::LargeUtf8(Some(v)) => v.clone(),
        ScalarValue::Utf8View(Some(v)) => v.clone(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// null_ratio
// ---------------------------------------------------------------------------

/// `null_ratio(expr) -> float64`
pub fn null_ratio_udaf() -> AggregateUDF {
    AggregateUDF::new_from_impl(NullRatioUdaf::new())
}

/// Aggregate-function definition for `null_ratio`.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct NullRatioUdaf {
    signature: Signature,
}

impl NullRatioUdaf {
    /// New definition.
    pub fn new() -> Self {
        Self {
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

impl Default for NullRatioUdaf {
    fn default() -> Self {
        Self::new()
    }
}

impl AggregateUDFImpl for NullRatioUdaf {
    fn name(&self) -> &str {
        "null_ratio"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> DfResult<DataType> {
        Ok(DataType::Float64)
    }

    fn accumulator(&self, _args: AccumulatorArgs) -> DfResult<Box<dyn Accumulator>> {
        Ok(Box::new(NullRatioAccumulator::default()))
    }

    fn state_fields(&self, _args: StateFieldsArgs) -> DfResult<Vec<Arc<Field>>> {
        Ok(vec![
            Arc::new(Field::new("total", DataType::UInt64, false)),
            Arc::new(Field::new("nulls", DataType::UInt64, false)),
        ])
    }
}

/// Counts total and null entries so the ratio can be merged.
#[derive(Debug, Default)]
struct NullRatioAccumulator {
    total: u64,
    nulls: u64,
}

impl Accumulator for NullRatioAccumulator {
    fn update_batch(&mut self, values: &[ArrayRef]) -> DfResult<()> {
        let array = values.first().ok_or_else(|| {
            DataFusionError::Execution("null_ratio: expected one argument".to_string())
        })?;
        self.total += array.len() as u64;
        self.nulls += array.null_count() as u64;
        Ok(())
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> DfResult<()> {
        if states.len() != 2 {
            return Err(DataFusionError::Execution(
                "null_ratio: expected two state columns".to_string(),
            ));
        }
        for (index, column) in states.iter().enumerate() {
            let column = column
                .as_any()
                .downcast_ref::<UInt64Array>()
                .ok_or_else(|| {
                    DataFusionError::Execution("null_ratio: state must be UInt64".to_string())
                })?;
            let sum: u64 = column.iter().flatten().sum();
            if index == 0 {
                self.total += sum;
            } else {
                self.nulls += sum;
            }
        }
        Ok(())
    }

    fn state(&mut self) -> DfResult<Vec<ScalarValue>> {
        Ok(vec![
            ScalarValue::UInt64(Some(self.total)),
            ScalarValue::UInt64(Some(self.nulls)),
        ])
    }

    fn evaluate(&mut self) -> DfResult<ScalarValue> {
        if self.total == 0 {
            Ok(ScalarValue::Float64(None))
        } else {
            Ok(ScalarValue::Float64(Some(
                self.nulls as f64 / self.total as f64,
            )))
        }
    }

    fn size(&self) -> usize {
        std::mem::size_of_val(self)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::arrow::array::{ArrayRef, Int64Array, StringArray, UInt64Array};

    use super::*;

    /// Run one batch through a fresh accumulator and hand back its state as arrays,
    /// exactly like the planner does between a partial and a final aggregation.
    fn partial_state(batch: ArrayRef) -> Vec<ArrayRef> {
        let mut accumulator = ModeAccumulator::default();
        accumulator.update_batch(&[batch]).unwrap();
        accumulator
            .state()
            .unwrap()
            .iter()
            .map(|scalar| scalar.to_array().unwrap())
            .collect()
    }

    #[test]
    fn mode_counts_values_and_ignores_nulls() {
        let values: ArrayRef = Arc::new(StringArray::from(vec![
            Some("a"),
            None,
            Some("b"),
            Some("b"),
            None,
            Some("a"),
            Some("b"),
        ]));
        let mut accumulator = ModeAccumulator::default();
        accumulator.update_batch(&[values]).unwrap();
        assert_eq!(
            accumulator.evaluate().unwrap(),
            ScalarValue::Utf8(Some("b".to_string()))
        );
    }

    #[test]
    fn mode_survives_a_partial_then_final_aggregation() {
        let left: ArrayRef = Arc::new(StringArray::from(vec!["x", "x", "y", "z"]));
        let right: ArrayRef = Arc::new(StringArray::from(vec!["y", "y", "z"]));

        // partial: x=2, y=1, z=1   partial: y=2, z=1  => merged y=3 wins
        let mut final_accumulator = ModeAccumulator::default();
        final_accumulator.merge_batch(&partial_state(left)).unwrap();
        final_accumulator
            .merge_batch(&partial_state(right))
            .unwrap();

        assert_eq!(
            final_accumulator.evaluate().unwrap(),
            ScalarValue::Utf8(Some("y".to_string()))
        );
    }

    #[test]
    fn mode_tie_breaks_are_deterministic() {
        let values: ArrayRef = Arc::new(StringArray::from(vec!["b", "a", "b", "a"]));
        let mut accumulator = ModeAccumulator::default();
        accumulator.update_batch(&[values]).unwrap();
        // equal counts: the smallest rendered value wins, not HashMap iteration order
        assert_eq!(
            accumulator.evaluate().unwrap(),
            ScalarValue::Utf8(Some("a".to_string()))
        );
    }

    #[test]
    fn mode_of_an_empty_group_is_null() {
        let mut accumulator = ModeAccumulator::default();
        assert_eq!(accumulator.evaluate().unwrap(), ScalarValue::Utf8(None));
    }

    #[test]
    fn mode_renders_numeric_values_as_text() {
        let values: ArrayRef = Arc::new(Int64Array::from(vec![7, 7, 9]));
        let mut accumulator = ModeAccumulator::default();
        accumulator.update_batch(&[values]).unwrap();
        assert_eq!(
            accumulator.evaluate().unwrap(),
            ScalarValue::Utf8(Some("7".to_string()))
        );
    }

    #[test]
    fn null_ratio_merges_partials() {
        let left: ArrayRef = Arc::new(StringArray::from(vec![Some("a"), None]));
        let right: ArrayRef = Arc::new(StringArray::from(vec![None, None, Some("c"), Some("d")]));

        let mut accumulator = NullRatioAccumulator::default();
        accumulator.update_batch(&[left]).unwrap();
        let state: Vec<ArrayRef> = accumulator
            .state()
            .unwrap()
            .iter()
            .map(|scalar| scalar.to_array().unwrap())
            .collect();
        assert_eq!(state.len(), 2);
        assert_eq!(
            state[0]
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .value(0),
            2
        );

        accumulator.merge_batch(&right_state(right)).unwrap();
        // 3 nulls out of 6 values
        assert_eq!(
            accumulator.evaluate().unwrap(),
            ScalarValue::Float64(Some(0.5))
        );
    }

    /// State of an independent partial aggregation over `batch`.
    fn right_state(batch: ArrayRef) -> Vec<ArrayRef> {
        let mut partial = NullRatioAccumulator::default();
        partial.update_batch(&[batch]).unwrap();
        partial
            .state()
            .unwrap()
            .iter()
            .map(|scalar| scalar.to_array().unwrap())
            .collect()
    }

    #[test]
    fn null_ratio_of_an_empty_group_is_null() {
        let mut accumulator = NullRatioAccumulator::default();
        assert_eq!(accumulator.evaluate().unwrap(), ScalarValue::Float64(None));
    }
}
