use std::cmp::max;

use itertools::izip;
use plonky2::field::extension::Extendable;
use plonky2::field::packed::PackedField;
use plonky2::field::types::Field;
use plonky2::hash::hash_types::RichField;
use plonky2::iop::ext_target::ExtensionTarget;

use crate::constraint_consumer::{ConstraintConsumer, RecursiveConstraintConsumer};
use crate::cpu::columns::ops::OpsColumnsView;
use crate::cpu::columns::CpuColumnsView;
use crate::cpu::membus::NUM_GP_CHANNELS;
use crate::memory::segments::Segment;

#[derive(Clone, Copy)]
pub(crate) struct StackBehavior {
    pub(crate) num_pops: usize,
    pub(crate) pushes: bool,
    new_top_stack_channel: Option<usize>,
    disable_other_channels: bool,
}

const BASIC_BINARY_OP: Option<StackBehavior> = Some(StackBehavior {
    num_pops: 2,
    pushes: true,
    new_top_stack_channel: Some(NUM_GP_CHANNELS - 1),
    disable_other_channels: true,
});
const BASIC_TERNARY_OP: Option<StackBehavior> = Some(StackBehavior {
    num_pops: 3,
    pushes: true,
    new_top_stack_channel: Some(NUM_GP_CHANNELS - 1),
    disable_other_channels: true,
});
pub(crate) const JUMP_OP: Option<StackBehavior> = Some(StackBehavior {
    num_pops: 1,
    pushes: false,
    new_top_stack_channel: None,
    disable_other_channels: false,
});
pub(crate) const JUMPI_OP: Option<StackBehavior> = Some(StackBehavior {
    num_pops: 2,
    pushes: false,
    new_top_stack_channel: None,
    disable_other_channels: false,
});

pub(crate) const MLOAD_GENERAL_OP: Option<StackBehavior> = Some(StackBehavior {
    num_pops: 3,
    pushes: true,
    new_top_stack_channel: None,
    disable_other_channels: false,
});

// AUDITORS: If the value below is `None`, then the operation must be manually checked to ensure
// that every general-purpose memory channel is either disabled or has its read flag and address
// propertly constrained. The same applies  when `disable_other_channels` is set to `false`,
// except the first `num_pops` and the last `pushes as usize` channels have their read flag and
// address constrained automatically in this file.
pub(crate) const STACK_BEHAVIORS: OpsColumnsView<Option<StackBehavior>> = OpsColumnsView {
    binary_op: BASIC_BINARY_OP,
    ternary_op: BASIC_TERNARY_OP,
    fp254_op: BASIC_BINARY_OP,
    eq_iszero: None, // EQ is binary, IS_ZERO is unary.
    logic_op: BASIC_BINARY_OP,
    not: Some(StackBehavior {
        num_pops: 1,
        pushes: true,
        new_top_stack_channel: Some(NUM_GP_CHANNELS - 1),
        disable_other_channels: true,
    }),
    shift: Some(StackBehavior {
        num_pops: 2,
        pushes: true,
        new_top_stack_channel: Some(NUM_GP_CHANNELS - 1),
        disable_other_channels: false,
    }),
    keccak_general: Some(StackBehavior {
        num_pops: 4,
        pushes: true,
        new_top_stack_channel: Some(NUM_GP_CHANNELS - 1),
        disable_other_channels: true,
    }),
    prover_input: None, // TODO
    pop: Some(StackBehavior {
        num_pops: 1,
        pushes: false,
        new_top_stack_channel: None,
        disable_other_channels: true,
    }),
    jumps: None, // Depends on whether it's a JUMP or a JUMPI.
    pc: Some(StackBehavior {
        num_pops: 0,
        pushes: true,
        new_top_stack_channel: None,
        disable_other_channels: true,
    }),
    jumpdest: Some(StackBehavior {
        num_pops: 0,
        pushes: false,
        new_top_stack_channel: None,
        disable_other_channels: true,
    }),
    push0: Some(StackBehavior {
        num_pops: 0,
        pushes: true,
        new_top_stack_channel: None,
        disable_other_channels: true,
    }),
    push: None, // TODO
    dup: None,
    swap: None,
    get_context: Some(StackBehavior {
        num_pops: 0,
        pushes: true,
        new_top_stack_channel: None,
        disable_other_channels: true,
    }),
    set_context: None, // SET_CONTEXT is special since it involves the old and the new stack.
    mload_32bytes: Some(StackBehavior {
        num_pops: 4,
        pushes: true,
        new_top_stack_channel: Some(4),
        disable_other_channels: false,
    }),
    mstore_32bytes: Some(StackBehavior {
        num_pops: 5,
        pushes: false,
        new_top_stack_channel: None,
        disable_other_channels: false,
    }),
    exit_kernel: Some(StackBehavior {
        num_pops: 1,
        pushes: false,
        new_top_stack_channel: None,
        disable_other_channels: true,
    }),
    m_op_general: None,
    syscall: Some(StackBehavior {
        num_pops: 0,
        pushes: true,
        new_top_stack_channel: None,
        disable_other_channels: false,
    }),
    exception: Some(StackBehavior {
        num_pops: 0,
        pushes: true,
        new_top_stack_channel: None,
        disable_other_channels: false,
    }),
};

pub(crate) const EQ_STACK_BEHAVIOR: Option<StackBehavior> = Some(StackBehavior {
    num_pops: 2,
    pushes: true,
    new_top_stack_channel: Some(2),
    disable_other_channels: true,
});
pub(crate) const IS_ZERO_STACK_BEHAVIOR: Option<StackBehavior> = Some(StackBehavior {
    num_pops: 1,
    pushes: true,
    new_top_stack_channel: Some(2),
    disable_other_channels: true,
});

pub(crate) fn eval_packed_one<P: PackedField>(
    lv: &CpuColumnsView<P>,
    nv: &CpuColumnsView<P>,
    filter: P,
    stack_behavior: StackBehavior,
    yield_constr: &mut ConstraintConsumer<P>,
) {
    // If you have pops.
    if stack_behavior.num_pops > 0 {
        for i in 1..stack_behavior.num_pops {
            let channel = lv.mem_channels[i];

            yield_constr.constraint(filter * (channel.used - P::ONES));
            yield_constr.constraint(filter * (channel.is_read - P::ONES));

            yield_constr.constraint(filter * (channel.addr_context - lv.context));
            yield_constr.constraint(
                filter
                    * (channel.addr_segment - P::Scalar::from_canonical_u64(Segment::Stack as u64)),
            );
            // Remember that the first read (`i == 1`) is for the second stack element at `stack[stack_len - 1]`.
            let addr_virtual = lv.stack_len - P::Scalar::from_canonical_usize(i + 1);
            yield_constr.constraint(filter * (channel.addr_virtual - addr_virtual));
        }

        // If you also push, you don't need to read the new top of the stack.
        // If you don't:
        // - if the stack isn't empty after the pops, you read the new top from an extra pop.
        // - if not, the extra read is disabled.
        // These are transition constraints: they don't apply to the last row.
        if !stack_behavior.pushes {
            // If stack_len != N...
            let len_diff = lv.stack_len - P::Scalar::from_canonical_usize(stack_behavior.num_pops);
            let new_filter = len_diff * filter;
            // Read an extra element.
            let channel = nv.mem_channels[0];
            yield_constr.constraint_transition(new_filter * (channel.used - P::ONES));
            yield_constr.constraint_transition(new_filter * (channel.is_read - P::ONES));
            yield_constr.constraint_transition(new_filter * (channel.addr_context - nv.context));
            yield_constr.constraint_transition(
                new_filter
                    * (channel.addr_segment - P::Scalar::from_canonical_u64(Segment::Stack as u64)),
            );
            let addr_virtual = nv.stack_len - P::ONES;
            yield_constr.constraint_transition(new_filter * (channel.addr_virtual - addr_virtual));
            // Constrain `stack_inv_aux`.
            yield_constr.constraint(
                filter
                    * (len_diff * lv.general.stack().stack_inv - lv.general.stack().stack_inv_aux),
            );
            // Disable channel if stack_len == N.
            let empty_stack_filter = filter * (lv.general.stack().stack_inv_aux - P::ONES);
            yield_constr.constraint_transition(empty_stack_filter * channel.used);
        }
    }
    // If the op only pushes, you only need to constrain the top of the stack if the stack isn't empty.
    else if stack_behavior.pushes {
        // If len > 0...
        let new_filter = lv.stack_len * filter;
        // You write the previous top of the stack in memory, in the last channel.
        let channel = lv.mem_channels[NUM_GP_CHANNELS - 1];
        yield_constr.constraint(new_filter * (channel.used - P::ONES));
        yield_constr.constraint(new_filter * channel.is_read);
        yield_constr.constraint(new_filter * (channel.addr_context - lv.context));
        yield_constr.constraint(
            new_filter
                * (channel.addr_segment - P::Scalar::from_canonical_u64(Segment::Stack as u64)),
        );
        let addr_virtual = lv.stack_len - P::ONES;
        yield_constr.constraint(new_filter * (channel.addr_virtual - addr_virtual));
        for (limb_ch, limb_top) in channel.value.iter().zip(lv.mem_channels[0].value.iter()) {
            yield_constr.constraint(new_filter * (*limb_ch - *limb_top));
        }
        // Else you disable the channel.
        yield_constr.constraint(
            filter
                * (lv.stack_len * lv.general.stack().stack_inv - lv.general.stack().stack_inv_aux),
        );
        let empty_stack_filter = filter * (lv.general.stack().stack_inv_aux - P::ONES);
        yield_constr.constraint(empty_stack_filter * channel.used);
    }
    // If the op doesn't pop nor push, the top of the stack must not change.
    else {
        yield_constr.constraint(filter * nv.mem_channels[0].used);
        for (limb_old, limb_new) in lv.mem_channels[0]
            .value
            .iter()
            .zip(nv.mem_channels[0].value.iter())
        {
            yield_constr.constraint(filter * (*limb_old - *limb_new));
        }
    }

    // Maybe constrain next stack_top.
    // These are transition constraints: they don't apply to the last row.
    if let Some(next_top_ch) = stack_behavior.new_top_stack_channel {
        for (limb_ch, limb_top) in lv.mem_channels[next_top_ch]
            .value
            .iter()
            .zip(nv.mem_channels[0].value.iter())
        {
            yield_constr.constraint_transition(filter * (*limb_ch - *limb_top));
        }
    }

    // Unused channels
    if stack_behavior.disable_other_channels {
        // The first channel contains (or not) the top od the stack and is constrained elsewhere.
        for i in max(1, stack_behavior.num_pops)..NUM_GP_CHANNELS - (stack_behavior.pushes as usize)
        {
            let channel = lv.mem_channels[i];
            yield_constr.constraint(filter * channel.used);
        }
    }

    // Constrain new stack length.
    let num_pops = P::Scalar::from_canonical_usize(stack_behavior.num_pops);
    let push = P::Scalar::from_canonical_usize(stack_behavior.pushes as usize);
    yield_constr.constraint_transition(filter * (nv.stack_len - (lv.stack_len - num_pops + push)));
}

pub fn eval_packed<P: PackedField>(
    lv: &CpuColumnsView<P>,
    nv: &CpuColumnsView<P>,
    yield_constr: &mut ConstraintConsumer<P>,
) {
    for (op, stack_behavior) in izip!(lv.op.into_iter(), STACK_BEHAVIORS.into_iter()) {
        if let Some(stack_behavior) = stack_behavior {
            eval_packed_one(lv, nv, op, stack_behavior, yield_constr);
        }
    }
}

pub(crate) fn eval_ext_circuit_one<F: RichField + Extendable<D>, const D: usize>(
    builder: &mut plonky2::plonk::circuit_builder::CircuitBuilder<F, D>,
    lv: &CpuColumnsView<ExtensionTarget<D>>,
    nv: &CpuColumnsView<ExtensionTarget<D>>,
    filter: ExtensionTarget<D>,
    stack_behavior: StackBehavior,
    yield_constr: &mut RecursiveConstraintConsumer<F, D>,
) {
    // If you have pops.
    if stack_behavior.num_pops > 0 {
        for i in 1..stack_behavior.num_pops {
            let channel = lv.mem_channels[i];

            {
                let constr = builder.mul_sub_extension(filter, channel.used, filter);
                yield_constr.constraint(builder, constr);
            }
            {
                let constr = builder.mul_sub_extension(filter, channel.is_read, filter);
                yield_constr.constraint(builder, constr);
            }
            {
                let diff = builder.sub_extension(channel.addr_context, lv.context);
                let constr = builder.mul_extension(filter, diff);
                yield_constr.constraint(builder, constr);
            }
            {
                let constr = builder.arithmetic_extension(
                    F::ONE,
                    -F::from_canonical_u64(Segment::Stack as u64),
                    filter,
                    channel.addr_segment,
                    filter,
                );
                yield_constr.constraint(builder, constr);
            }
            // Remember that the first read (`i == 1`) is for the second stack element at `stack[stack_len - 1]`.
            {
                let diff = builder.sub_extension(channel.addr_virtual, lv.stack_len);
                let constr = builder.arithmetic_extension(
                    F::ONE,
                    F::from_canonical_usize(i + 1),
                    filter,
                    diff,
                    filter,
                );
                yield_constr.constraint(builder, constr);
            }
        }

        // If you also push, you don't need to read the new top of the stack.
        // If you don't:
        // - if the stack isn't empty after the pops, you read the new top from an extra pop.
        // - if not, the extra read is disabled.
        // These are transition constraints: they don't apply to the last row.
        if !stack_behavior.pushes {
            // If stack_len != N...
            let target_num_pops =
                builder.constant_extension(F::from_canonical_usize(stack_behavior.num_pops).into());
            let len_diff = builder.sub_extension(lv.stack_len, target_num_pops);
            let new_filter = builder.mul_extension(filter, len_diff);
            // Read an extra element.
            let channel = nv.mem_channels[0];

            {
                let constr = builder.mul_sub_extension(new_filter, channel.used, new_filter);
                yield_constr.constraint_transition(builder, constr);
            }
            {
                let constr = builder.mul_sub_extension(new_filter, channel.is_read, new_filter);
                yield_constr.constraint_transition(builder, constr);
            }
            {
                let diff = builder.sub_extension(channel.addr_context, nv.context);
                let constr = builder.mul_extension(new_filter, diff);
                yield_constr.constraint_transition(builder, constr);
            }
            {
                let constr = builder.arithmetic_extension(
                    F::ONE,
                    -F::from_canonical_u64(Segment::Stack as u64),
                    new_filter,
                    channel.addr_segment,
                    new_filter,
                );
                yield_constr.constraint_transition(builder, constr);
            }
            {
                let diff = builder.sub_extension(channel.addr_virtual, nv.stack_len);
                let constr =
                    builder.arithmetic_extension(F::ONE, F::ONE, new_filter, diff, new_filter);
                yield_constr.constraint_transition(builder, constr);
            }
            // Constrain `stack_inv_aux`.
            {
                let prod = builder.mul_extension(len_diff, lv.general.stack().stack_inv);
                let diff = builder.sub_extension(prod, lv.general.stack().stack_inv_aux);
                let constr = builder.mul_extension(filter, diff);
                yield_constr.constraint(builder, constr);
            }
            // Disable channel if stack_len == N.
            {
                let empty_stack_filter =
                    builder.mul_sub_extension(filter, lv.general.stack().stack_inv_aux, filter);
                let constr = builder.mul_extension(empty_stack_filter, channel.used);
                yield_constr.constraint_transition(builder, constr);
            }
        }
    }
    // If the op only pushes, you only need to constrain the top of the stack if the stack isn't empty.
    else if stack_behavior.pushes {
        // If len > 0...
        let new_filter = builder.mul_extension(lv.stack_len, filter);
        // You write the previous top of the stack in memory, in the last channel.
        let channel = lv.mem_channels[NUM_GP_CHANNELS - 1];
        {
            let constr = builder.mul_sub_extension(new_filter, channel.used, new_filter);
            yield_constr.constraint(builder, constr);
        }
        {
            let constr = builder.mul_extension(new_filter, channel.is_read);
            yield_constr.constraint(builder, constr);
        }

        {
            let diff = builder.sub_extension(channel.addr_context, lv.context);
            let constr = builder.mul_extension(new_filter, diff);
            yield_constr.constraint(builder, constr);
        }
        {
            let constr = builder.arithmetic_extension(
                F::ONE,
                -F::from_canonical_u64(Segment::Stack as u64),
                new_filter,
                channel.addr_segment,
                new_filter,
            );
            yield_constr.constraint(builder, constr);
        }
        {
            let diff = builder.sub_extension(channel.addr_virtual, lv.stack_len);
            let constr = builder.arithmetic_extension(F::ONE, F::ONE, new_filter, diff, new_filter);
            yield_constr.constraint(builder, constr);
        }
        for (limb_ch, limb_top) in channel.value.iter().zip(lv.mem_channels[0].value.iter()) {
            let diff = builder.sub_extension(*limb_ch, *limb_top);
            let constr = builder.mul_extension(new_filter, diff);
            yield_constr.constraint(builder, constr);
        }
        // Else you disable the channel.
        {
            let diff = builder.mul_extension(lv.stack_len, lv.general.stack().stack_inv);
            let diff = builder.sub_extension(diff, lv.general.stack().stack_inv_aux);
            let constr = builder.mul_extension(filter, diff);
            yield_constr.constraint(builder, constr);
        }
        {
            let empty_stack_filter =
                builder.mul_sub_extension(filter, lv.general.stack().stack_inv_aux, filter);
            let constr = builder.mul_extension(empty_stack_filter, channel.used);
            yield_constr.constraint(builder, constr);
        }
    }
    // If the op doesn't pop nor push, the top of the stack must not change.
    else {
        {
            let constr = builder.mul_extension(filter, nv.mem_channels[0].used);
            yield_constr.constraint(builder, constr);
        }
        {
            for (limb_old, limb_new) in lv.mem_channels[0]
                .value
                .iter()
                .zip(nv.mem_channels[0].value.iter())
            {
                let diff = builder.sub_extension(*limb_old, *limb_new);
                let constr = builder.mul_extension(filter, diff);
                yield_constr.constraint(builder, constr);
            }
        }
    }

    // Maybe constrain next stack_top.
    // These are transition constraints: they don't apply to the last row.
    if let Some(next_top_ch) = stack_behavior.new_top_stack_channel {
        for (limb_ch, limb_top) in lv.mem_channels[next_top_ch]
            .value
            .iter()
            .zip(nv.mem_channels[0].value.iter())
        {
            let diff = builder.sub_extension(*limb_ch, *limb_top);
            let constr = builder.mul_extension(filter, diff);
            yield_constr.constraint_transition(builder, constr);
        }
    }

    // Unused channels
    if stack_behavior.disable_other_channels {
        // The first channel contains (or not) the top od the stack and is constrained elsewhere.
        for i in max(1, stack_behavior.num_pops)..NUM_GP_CHANNELS - (stack_behavior.pushes as usize)
        {
            let channel = lv.mem_channels[i];
            let constr = builder.mul_extension(filter, channel.used);
            yield_constr.constraint(builder, constr);
        }
    }

    // Constrain new stack length.
    let diff = builder.constant_extension(
        F::Extension::from_canonical_usize(stack_behavior.num_pops)
            - F::Extension::from_canonical_usize(stack_behavior.pushes as usize),
    );
    let diff = builder.sub_extension(lv.stack_len, diff);
    let diff = builder.sub_extension(nv.stack_len, diff);
    let constr = builder.mul_extension(filter, diff);
    yield_constr.constraint_transition(builder, constr);
}

pub fn eval_ext_circuit<F: RichField + Extendable<D>, const D: usize>(
    builder: &mut plonky2::plonk::circuit_builder::CircuitBuilder<F, D>,
    lv: &CpuColumnsView<ExtensionTarget<D>>,
    nv: &CpuColumnsView<ExtensionTarget<D>>,
    yield_constr: &mut RecursiveConstraintConsumer<F, D>,
) {
    for (op, stack_behavior) in izip!(lv.op.into_iter(), STACK_BEHAVIORS.into_iter()) {
        if let Some(stack_behavior) = stack_behavior {
            eval_ext_circuit_one(builder, lv, nv, op, stack_behavior, yield_constr);
        }
    }
}
