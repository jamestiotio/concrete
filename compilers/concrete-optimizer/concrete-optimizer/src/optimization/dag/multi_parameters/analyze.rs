use crate::dag::operator::{dot_kind, DotKind, LevelledComplexity, Operator, OperatorIndex, Shape};
use crate::dag::rewrite::round::expand_round;
use crate::dag::unparametrized;
use crate::optimization::config::NoiseBoundConfig;
use crate::optimization::dag::multi_parameters::partitionning::partitionning_with_preferred;
use crate::optimization::dag::multi_parameters::partitions::{
    InstructionPartition, PartitionIndex,
};
use crate::optimization::dag::multi_parameters::precision_cut::PrecisionCut;
use crate::optimization::dag::multi_parameters::symbolic_variance::SymbolicVariance;
use crate::optimization::dag::solo_key::analyze::first;
use crate::utils::square;

// private short convention
use DotKind as DK;

type Op = Operator;

pub struct AnalyzedDag {
    pub operators: Vec<Op>,
    // Collect all operators ouput variances
    pub nb_partitions: usize,
    pub instrs_partition: Vec<InstructionPartition>,
    pub out_variances: Vec<Vec<SymbolicVariance>>,
    // The full dag levelled complexity
    pub levelled_complexity: LevelledComplexity,
}

pub fn analyze(
    dag: &unparametrized::OperationDag,
    _noise_config: &NoiseBoundConfig,
    p_cut: &PrecisionCut,
    default_partition: PartitionIndex,
) -> AnalyzedDag {
    assert!(
        p_cut.p_cut.len() <= 1,
        "Multi-parameter can only be used 0 or 1 precision cut"
    );
    let dag = expand_round(dag);
    let levelled_complexity = LevelledComplexity::ZERO;
    // The precision cut is chosen to work well with rounded pbs
    // Note: this is temporary
    let partitions = partitionning_with_preferred(&dag, p_cut, default_partition);
    let instrs_partition = partitions.instrs_partition;
    let nb_partitions = partitions.nb_partitions;
    let out_variances = out_variances(&dag, nb_partitions, &instrs_partition);

    AnalyzedDag {
        operators: dag.operators,
        nb_partitions,
        instrs_partition,
        out_variances,
        levelled_complexity,
    }
}

fn out_variance(
    op: &unparametrized::UnparameterizedOperator,
    out_shapes: &[Shape],
    out_variances: &mut Vec<Vec<SymbolicVariance>>,
    nb_partitions: usize,
    instr_partition: &InstructionPartition,
) -> Vec<SymbolicVariance> {
    // one variance per partition, in case the result is converted
    let partition = instr_partition.instruction_partition;
    let out_variance_of = |input: &OperatorIndex| {
        assert!(input.i < out_variances.len());
        assert!(partition < out_variances[input.i].len());
        assert!(out_variances[input.i][partition] != SymbolicVariance::ZERO);
        assert!(!out_variances[input.i][partition].coeffs.values[0].is_nan());
        assert!(out_variances[input.i][partition].partition != usize::MAX);
        out_variances[input.i][partition].clone()
    };
    let max_variance = |acc: SymbolicVariance, input: SymbolicVariance| acc.max(&input);
    let variance = match op {
        Op::Input { .. } => SymbolicVariance::input(nb_partitions, partition),
        Op::Lut { .. } => SymbolicVariance::after_pbs(nb_partitions, partition),
        Op::LevelledOp { inputs, manp, .. } => {
            let inputs_variance = inputs.iter().map(out_variance_of);
            let max_variance = inputs_variance.reduce(max_variance).unwrap();
            max_variance.after_levelled_op(*manp)
        }
        Op::Dot {
            inputs, weights, ..
        } => {
            let input_shape = first(inputs, out_shapes);
            let kind = dot_kind(inputs.len() as u64, input_shape, weights);
            match kind {
                DK::Simple | DK::Tensor | DK::Broadcast => {
                    let inputs_variance = (0..weights.values.len()).map(|j| {
                        let input = if inputs.len() > 1 {
                            inputs[j]
                        } else {
                            inputs[0]
                        };
                        out_variance_of(&input)
                    });
                    let mut out_variance = SymbolicVariance::ZERO;
                    for (input_variance, &weight) in inputs_variance.zip(&weights.values) {
                        assert!(input_variance != SymbolicVariance::ZERO);
                        out_variance += input_variance * square(weight);
                    }
                    out_variance
                }
                DK::CompatibleTensor { .. } => todo!("TODO"),
                DK::Unsupported { .. } => panic!("Unsupported"),
            }
        }
        Op::UnsafeCast { input, .. } => out_variance_of(input),
        Op::Round { .. } => {
            unreachable!("Round should have been either expanded or integrated to a lut")
        }
    };
    // Injecting NAN in unused symbolic variance to detect bad use
    let unused = SymbolicVariance::nan(nb_partitions);
    let mut result = vec![unused; nb_partitions];
    for &dst_partition in &instr_partition.alternative_output_representation {
        let src_partition = partition;
        // make converted variance available in dst_partition
        result[dst_partition] =
            variance.after_partition_keyswitch_to_big(src_partition, dst_partition);
    }
    result[partition] = variance;
    result
}

fn out_variances(
    dag: &unparametrized::OperationDag,
    nb_partitions: usize,
    instrs_partition: &[InstructionPartition],
) -> Vec<Vec<SymbolicVariance>> {
    let nb_ops = dag.operators.len();
    let mut out_variances = Vec::with_capacity(nb_ops);
    for (op, instr_partition) in dag.operators.iter().zip(instrs_partition) {
        let vf = out_variance(
            op,
            &dag.out_shapes,
            &mut out_variances,
            nb_partitions,
            instr_partition,
        );
        out_variances.push(vf);
    }
    out_variances
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::operator::{FunctionTable, Shape};
    use crate::dag::unparametrized;
    use crate::optimization::dag::multi_parameters::partitionning::tests::{
        show_partitionning, HIGH_PRECISION_PARTITION, LOW_PRECISION_PARTITION,
    };
    use crate::optimization::dag::solo_key::analyze::tests::CONFIG;

    fn analyze(dag: &unparametrized::OperationDag) -> AnalyzedDag {
        analyze_with_preferred(dag, LOW_PRECISION_PARTITION)
    }

    fn analyze_with_preferred(
        dag: &unparametrized::OperationDag,
        default_partition: PartitionIndex,
    ) -> AnalyzedDag {
        let p_cut = PrecisionCut { p_cut: vec![2] };
        super::analyze(dag, &CONFIG, &p_cut, default_partition)
    }

    #[allow(clippy::float_cmp)]
    fn assert_input_on(dag: &AnalyzedDag, partition: usize, op_i: usize, expected_coeff: f64) {
        for symbolic_variance_partition in [LOW_PRECISION_PARTITION, HIGH_PRECISION_PARTITION] {
            let sb = dag.out_variances[op_i][partition].clone();
            let coeff = if sb == SymbolicVariance::ZERO {
                0.0
            } else {
                sb.coeff_input(symbolic_variance_partition)
            };
            if symbolic_variance_partition == partition {
                assert!(
                    coeff == expected_coeff,
                    "INCORRECT INPUT COEFF ON GOOD PARTITION {:?} {:?} {} {}",
                    dag.out_variances[op_i],
                    partition,
                    coeff,
                    expected_coeff
                );
            } else {
                assert!(
                    coeff == 0.0,
                    "INCORRECT INPUT COEFF ON WRONG PARTITION {:?} {:?} {} {}",
                    dag.out_variances[op_i],
                    partition,
                    coeff,
                    expected_coeff
                );
            }
        }
    }

    #[allow(clippy::float_cmp)]
    fn assert_pbs_on(dag: &AnalyzedDag, partition: usize, op_i: usize, expected_coeff: f64) {
        for symbolic_variance_partition in [LOW_PRECISION_PARTITION, HIGH_PRECISION_PARTITION] {
            let sb = dag.out_variances[op_i][partition].clone();
            eprintln!("{:?}", dag.out_variances[op_i]);
            eprintln!("{:?}", dag.out_variances[op_i][partition]);
            let coeff = if sb == SymbolicVariance::ZERO {
                0.0
            } else {
                sb.coeff_pbs(symbolic_variance_partition)
            };
            if symbolic_variance_partition == partition {
                assert!(
                    coeff == expected_coeff,
                    "INCORRECT PBS COEFF ON GOOD PARTITION {:?} {:?} {} {}",
                    dag.out_variances[op_i],
                    partition,
                    coeff,
                    expected_coeff
                );
            } else {
                assert!(
                    coeff == 0.0,
                    "INCORRECT PBS COEFF ON GOOD PARTITION {:?} {:?} {} {}",
                    dag.out_variances[op_i],
                    partition,
                    coeff,
                    expected_coeff
                );
            }
        }
    }

    #[allow(clippy::needless_range_loop)]
    #[test]
    fn test_lut_sequence() {
        let mut dag = unparametrized::OperationDag::new();
        let input1 = dag.add_input(8, Shape::number());
        let lut1 = dag.add_lut(input1, FunctionTable::UNKWOWN, 8);
        let lut2 = dag.add_lut(lut1, FunctionTable::UNKWOWN, 1);
        let lut3 = dag.add_lut(lut2, FunctionTable::UNKWOWN, 1);
        let lut4 = dag.add_lut(lut3, FunctionTable::UNKWOWN, 8);
        let lut5 = dag.add_lut(lut4, FunctionTable::UNKWOWN, 8);
        let partitions = [
            HIGH_PRECISION_PARTITION,
            HIGH_PRECISION_PARTITION,
            HIGH_PRECISION_PARTITION,
            LOW_PRECISION_PARTITION,
            LOW_PRECISION_PARTITION,
            HIGH_PRECISION_PARTITION,
        ];
        let dag = analyze(&dag);
        assert!(dag.nb_partitions == 2);
        for op_i in input1.i..=lut5.i {
            let p = &dag.instrs_partition[op_i];
            let is_input = op_i == input1.i;
            assert!(p.instruction_partition == partitions[op_i]);
            if is_input {
                assert_input_on(&dag, p.instruction_partition, op_i, 1.0);
                assert_pbs_on(&dag, p.instruction_partition, op_i, 0.0);
            } else {
                assert_pbs_on(&dag, p.instruction_partition, op_i, 1.0);
                assert_input_on(&dag, p.instruction_partition, op_i, 0.0);
            }
        }
    }

    #[test]
    fn test_levelled_op() {
        let mut dag = unparametrized::OperationDag::new();
        let out_shape = Shape::number();
        let manp = 8.0;
        let input1 = dag.add_input(8, Shape::number());
        let input2 = dag.add_input(8, Shape::number());
        let lut1 = dag.add_lut(input1, FunctionTable::UNKWOWN, 8);
        let _levelled = dag.add_levelled_op(
            [lut1, input2],
            LevelledComplexity::ZERO,
            manp,
            &out_shape,
            "comment",
        );
        let dag = analyze(&dag);
        assert!(dag.nb_partitions == 1);
    }

    fn nan_symbolic_variance(sb: &SymbolicVariance) -> bool {
        sb.coeffs[0].is_nan()
    }

    #[allow(clippy::float_cmp)]
    #[test]
    fn test_rounded_v3_first_layer_and_second_layer() {
        let acc_precision = 16;
        let precision = 8;
        let mut dag = unparametrized::OperationDag::new();
        let input1 = dag.add_input(acc_precision, Shape::number());
        let rounded1 = dag.add_expanded_round(input1, precision);
        let lut1 = dag.add_lut(rounded1, FunctionTable::UNKWOWN, acc_precision);
        let rounded2 = dag.add_expanded_round(lut1, precision);
        let lut2 = dag.add_lut(rounded2, FunctionTable::UNKWOWN, acc_precision);
        let old_dag = dag;
        let dag = analyze(&old_dag);
        show_partitionning(&old_dag, &dag.instrs_partition);
        // First layer is fully LOW_PRECISION_PARTITION
        for op_i in input1.i..lut1.i {
            let p = LOW_PRECISION_PARTITION;
            let sb = &dag.out_variances[op_i][p];
            assert!(sb.coeff_input(p) >= 1.0 || sb.coeff_pbs(p) >= 1.0);
            assert!(nan_symbolic_variance(
                &dag.out_variances[op_i][HIGH_PRECISION_PARTITION]
            ));
        }
        // First lut is HIGH_PRECISION_PARTITION and immedialtely converted to LOW_PRECISION_PARTITION
        let p = HIGH_PRECISION_PARTITION;
        let sb = &dag.out_variances[lut1.i][p];
        assert!(sb.coeff_input(p) == 0.0);
        assert!(sb.coeff_pbs(p) == 1.0);
        let sb_after_fast_ks = &dag.out_variances[lut1.i][LOW_PRECISION_PARTITION];
        assert!(
            sb_after_fast_ks.coeff_partition_keyswitch_to_big(
                HIGH_PRECISION_PARTITION,
                LOW_PRECISION_PARTITION
            ) == 1.0
        );
        // The next rounded is on LOW_PRECISION_PARTITION but base noise can comes from HIGH_PRECISION_PARTITION + FKS
        for op_i in (lut1.i + 1)..lut2.i {
            assert!(LOW_PRECISION_PARTITION == dag.instrs_partition[op_i].instruction_partition);
            let p = LOW_PRECISION_PARTITION;
            let sb = &dag.out_variances[op_i][p];
            // The base noise is either from the other partition and shifted or from the current partition and 1
            assert!(sb.coeff_input(LOW_PRECISION_PARTITION) == 0.0);
            assert!(sb.coeff_input(HIGH_PRECISION_PARTITION) == 0.0);
            if sb.coeff_pbs(HIGH_PRECISION_PARTITION) >= 1.0 {
                assert!(
                    sb.coeff_pbs(HIGH_PRECISION_PARTITION)
                        == sb.coeff_partition_keyswitch_to_big(
                            HIGH_PRECISION_PARTITION,
                            LOW_PRECISION_PARTITION
                        )
                );
            } else {
                assert!(sb.coeff_pbs(LOW_PRECISION_PARTITION) == 1.0);
                assert!(
                    sb.coeff_partition_keyswitch_to_big(
                        HIGH_PRECISION_PARTITION,
                        LOW_PRECISION_PARTITION
                    ) == 0.0
                );
            }
        }
        assert!(nan_symbolic_variance(
            &dag.out_variances[lut2.i][LOW_PRECISION_PARTITION]
        ));
        let sb = &dag.out_variances[lut2.i][HIGH_PRECISION_PARTITION];
        assert!(sb.coeff_pbs(HIGH_PRECISION_PARTITION) >= 1.0);
    }

    #[allow(clippy::float_cmp, clippy::cognitive_complexity)]
    #[test]
    fn test_rounded_v3_classic_first_layer_second_layer() {
        let acc_precision = 16;
        let precision = 8;
        let mut dag = unparametrized::OperationDag::new();
        let free_input1 = dag.add_input(precision, Shape::number());
        let input1 = dag.add_lut(free_input1, FunctionTable::UNKWOWN, acc_precision);
        let rounded1 = dag.add_expanded_round(input1, precision);
        let _lut1 = dag.add_lut(rounded1, FunctionTable::UNKWOWN, acc_precision);
        let old_dag = dag;
        let dag = analyze(&old_dag);
        show_partitionning(&old_dag, &dag.instrs_partition);
        // First layer is fully HIGH_PRECISION_PARTITION
        assert!(
            dag.out_variances[free_input1.i][HIGH_PRECISION_PARTITION]
                .coeff_input(HIGH_PRECISION_PARTITION)
                == 1.0
        );
        // First layer tlu
        let sb = &dag.out_variances[input1.i][HIGH_PRECISION_PARTITION];
        assert!(sb.coeff_input(LOW_PRECISION_PARTITION) == 0.0);
        assert!(sb.coeff_pbs(HIGH_PRECISION_PARTITION) == 1.0);
        assert!(
            sb.coeff_partition_keyswitch_to_big(HIGH_PRECISION_PARTITION, LOW_PRECISION_PARTITION)
                == 0.0
        );
        // The same cyphertext exists in another partition with additional noise due to fast keyswitch
        let sb = &dag.out_variances[input1.i][LOW_PRECISION_PARTITION];
        assert!(sb.coeff_input(LOW_PRECISION_PARTITION) == 0.0);
        assert!(sb.coeff_pbs(HIGH_PRECISION_PARTITION) == 1.0);
        assert!(
            sb.coeff_partition_keyswitch_to_big(HIGH_PRECISION_PARTITION, LOW_PRECISION_PARTITION)
                == 1.0
        );

        // Second layer
        let mut first_bit_extract_verified = false;
        let mut first_bit_erase_verified = false;
        for op_i in (input1.i + 1)..rounded1.i {
            if let Op::Dot {
                weights, inputs, ..
            } = &dag.operators[op_i]
            {
                let bit_extract = weights.values.len() == 1;
                let first_bit_extract = bit_extract && !first_bit_extract_verified;
                let bit_erase = weights.values == [1, -1];
                let first_bit_erase = bit_erase && !first_bit_erase_verified;
                let input0_sb = &dag.out_variances[inputs[0].i][LOW_PRECISION_PARTITION];
                let input0_coeff_pbs_high = input0_sb.coeff_pbs(HIGH_PRECISION_PARTITION);
                let input0_coeff_pbs_low = input0_sb.coeff_pbs(LOW_PRECISION_PARTITION);
                let input0_coeff_fks = input0_sb.coeff_partition_keyswitch_to_big(
                    HIGH_PRECISION_PARTITION,
                    LOW_PRECISION_PARTITION,
                );
                if bit_extract {
                    first_bit_extract_verified |= first_bit_extract;
                    assert!(input0_coeff_pbs_high >= 1.0);
                    if first_bit_extract {
                        assert!(input0_coeff_pbs_low == 0.0);
                    } else {
                        assert!(input0_coeff_pbs_low >= 1.0);
                    }
                    assert!(input0_coeff_fks == 1.0);
                } else if bit_erase {
                    first_bit_erase_verified |= first_bit_erase;
                    let input1_sb = &dag.out_variances[inputs[1].i][LOW_PRECISION_PARTITION];
                    let input1_coeff_pbs_high = input1_sb.coeff_pbs(HIGH_PRECISION_PARTITION);
                    let input1_coeff_pbs_low = input1_sb.coeff_pbs(LOW_PRECISION_PARTITION);
                    let input1_coeff_fks = input1_sb.coeff_partition_keyswitch_to_big(
                        HIGH_PRECISION_PARTITION,
                        LOW_PRECISION_PARTITION,
                    );
                    if first_bit_erase {
                        assert!(input0_coeff_pbs_low == 0.0);
                    } else {
                        assert!(input0_coeff_pbs_low >= 1.0);
                    }
                    assert!(input0_coeff_pbs_high == 1.0);
                    assert!(input0_coeff_fks == 1.0);
                    assert!(input1_coeff_pbs_low == 1.0);
                    assert!(input1_coeff_pbs_high == 0.0);
                    assert!(input1_coeff_fks == 0.0);
                }
            }
        }
        assert!(first_bit_extract_verified);
        assert!(first_bit_erase_verified);
    }
}