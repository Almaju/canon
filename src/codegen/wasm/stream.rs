//! `Stream<String>` at the value level: a pull-based chunk source.
//!
//! A stream is a pointer to a 24-byte *stage*: the slot of its `next`
//! function in the module's funcref table, three i32 cells the stage
//! kind reads and writes, and eight bytes of padding the layout keeps
//! aligned. `$stream_next(stage) -> (ptr, len, has)` does one
//! `call_indirect` through the slot; `has` is 0 at the end, when the
//! chunk is empty. Every consumer (`Folded`, `First`, the drain into a
//! `String`) pulls through it, and every producer builds a stage:
//!
//!   - `Done` — slot 0, always present: the end, and what a finished
//!     stage rewrites its own slot to.
//!   - `List` — the slots of a `List<String>` in turn.
//!   - `Host` — a `wasi:*` byte stream read chunk by chunk through the
//!     canonical `stream.read`, the handles dropped at its end.
//!   - `Taken` — at most N chunks of an inner stage.
//!   - `Map` — each chunk of an inner stage through an inlined lambda.
//!
//! A `Map` stage is one function per lambda site (Canon lambdas are
//! non-capturing, so the body compiles in the stage's own frame with the
//! chunk bound to its parameter); the other kinds are one function per
//! module, or per extern for `Host`, deduplicated by `Stage::same`.
use std::borrow::Cow;

use wasm_encoder::{ElementSection, Elements, RefType, TableSection, TableType};

use super::*;

/// Stage layout: the table slot, three cells, 8 bytes of padding.
const OFF_SLOT: u64 = 0;
const OFF_A: u64 = 4;
const OFF_B: u64 = 8;
const OFF_C: u64 = 12;
const STAGE_SIZE: i32 = 24;
/// One host read's room, matching the drains' chunk.
const CHUNK: i32 = 65536;

#[derive(Clone, Debug)]
pub(super) enum Stage {
    Done,
    /// A = the current slot's address, B = slots remaining.
    List,
    /// A = the inner stage, B = chunks remaining.
    Taken,
    /// A = the stream handle, B = its completion future, C = what
    /// `third` says.
    Host {
        read_fn: u32,
        drop_stream_fn: u32,
        drop_future_fn: u32,
        third: Third,
    },
    /// A = the inner stage; `param` binds each chunk in `body`.
    Map {
        param: String,
        body: Block,
        site: crate::error::Span,
    },
}

/// The third cell of a `Host` stage, settled with the handles at the
/// stream's end.
#[derive(Clone, Copy, Debug)]
pub(super) enum Third {
    Nothing,
    /// The file descriptor the stream reads, dropped.
    Descriptor {
        drop_fn: u32,
    },
    /// The writer of the `res` future a body was consumed with:
    /// resolved to `ok` (the eight zero bytes at `ok_at`), then dropped.
    Settled {
        write_fn: u32,
        drop_fn: u32,
        ok_at: u32,
    },
}

impl Stage {
    fn same(&self, other: &Stage) -> bool {
        match (self, other) {
            (Stage::Done, Stage::Done)
            | (Stage::List, Stage::List)
            | (Stage::Taken, Stage::Taken) => true,
            (Stage::Host { read_fn: a, .. }, Stage::Host { read_fn: b, .. }) => a == b,
            (Stage::Map { site: a, .. }, Stage::Map { site: b, .. }) => a == b,
            _ => false,
        }
    }
}

fn mem32(offset: u64) -> MemArg {
    MemArg {
        offset,
        align: 2,
        memory_index: 0,
    }
}

/// `(0, 0, 0)` — the end — and return.
fn emit_end_return(f: &mut Function) {
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::Return);
}

impl<'m> WasmGen<'m> {
    /// The table slot of `stage`, registering it when no equal stage is
    /// known yet.
    pub(super) fn stream_stage(&mut self, stage: Stage) -> u32 {
        if let Some(slot) = self.stream_stages.iter().position(|s| s.same(&stage)) {
            return slot as u32;
        }
        self.stream_stages.push(stage);
        (self.stream_stages.len() - 1) as u32
    }

    /// The stage functions' type, `(stage) -> (ptr, len, has)`, which
    /// the assemblers reserve before any body compiles.
    pub(super) fn stream_stage_ty(&self) -> u32 {
        self.user_type_map
            .get(&(vec![ValType::I32], vec![ValType::I32; 3]))
            .copied()
            // invariant: every assembler reserves the stage type in
            // `user_type_map` before compiling a body.
            .expect("stream stage type reserved before bodies compile")
    }

    /// A pointer whose type is `Stream` or a newtype chain ending at one
    /// (`Stdin = Stream<String>`).
    pub(super) fn is_stream_ty(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::NamedPtr(n) if self.collect_alias_chain(n).iter().any(|a| a == "Stream"))
    }

    /// `$stream_next(stage) -> (ptr, len, has)`: one `call_indirect`
    /// through the stage's slot.
    pub(super) fn build_stream_next(&self) -> Function {
        let mut f = Function::new([]);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32Load(mem32(OFF_SLOT)));
        f.instruction(&Instruction::CallIndirect {
            type_index: self.stream_stage_ty(),
            table_index: 0,
        });
        f.instruction(&Instruction::End);
        f
    }

    /// Every stage function in slot order. A `Map` body may register
    /// further stages while it compiles, so the list is walked to its
    /// end rather than snapshotted.
    pub(super) fn build_stream_bodies(&mut self) -> Vec<Function> {
        let mut out = Vec::new();
        while out.len() < self.stream_stages.len() {
            let stage = self.stream_stages[out.len()].clone();
            out.push(self.build_stage(&stage));
        }
        out
    }

    /// The funcref table the stages fill, exactly their number.
    pub(super) fn stream_table_section(&self) -> TableSection {
        let mut tables = TableSection::new();
        let n = self.stream_stages.len() as u64;
        tables.table(TableType {
            element_type: RefType::FUNCREF,
            table64: false,
            minimum: n,
            maximum: Some(n),
            shared: false,
        });
        tables
    }

    /// Slot `i` holds stage function `i`, defined right after
    /// `$stream_next`.
    pub(super) fn stream_element_section(&self) -> ElementSection {
        let mut elements = ElementSection::new();
        let funcs: Vec<u32> = (0..self.stream_stages.len() as u32)
            .map(|i| self.fn_stream_next + 1 + i)
            .collect();
        elements.active(
            Some(0),
            &ConstExpr::i32_const(0),
            Elements::Functions(Cow::Owned(funcs)),
        );
        elements
    }

    fn build_stage(&mut self, stage: &Stage) -> Function {
        let scope = LocalScope {
            vars: HashMap::new(),
            param_count: 1,
            arm_depth: 0,
        };
        let arm_depth = match stage {
            Stage::Map { body, .. } => max_arm_depth(body),
            _ => 0,
        };
        let mut f = Function::new(extra_locals_decl(arm_depth));
        match stage {
            Stage::Done => emit_end_return(&mut f),
            Stage::List => {
                f.instruction(&Instruction::LocalGet(0));
                f.instruction(&Instruction::I32Load(mem32(OFF_B)));
                f.instruction(&Instruction::LocalTee(scope.tmp_i32()));
                f.instruction(&Instruction::I32Eqz);
                f.instruction(&Instruction::If(BlockType::Empty));
                emit_end_return(&mut f);
                f.instruction(&Instruction::End);
                f.instruction(&Instruction::LocalGet(0));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::I32Sub);
                f.instruction(&Instruction::I32Store(mem32(OFF_B)));
                f.instruction(&Instruction::LocalGet(0));
                f.instruction(&Instruction::I32Load(mem32(OFF_A)));
                f.instruction(&Instruction::LocalSet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(0));
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::I32Store(mem32(OFF_A)));
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::I32Load(mem32(0)));
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::I32Load(mem32(4)));
                f.instruction(&Instruction::I32Const(1));
            }
            Stage::Taken => {
                f.instruction(&Instruction::LocalGet(0));
                f.instruction(&Instruction::I32Load(mem32(OFF_B)));
                f.instruction(&Instruction::LocalTee(scope.tmp_i32()));
                f.instruction(&Instruction::I32Eqz);
                f.instruction(&Instruction::If(BlockType::Empty));
                emit_end_return(&mut f);
                f.instruction(&Instruction::End);
                f.instruction(&Instruction::LocalGet(0));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::I32Sub);
                f.instruction(&Instruction::I32Store(mem32(OFF_B)));
                f.instruction(&Instruction::LocalGet(0));
                f.instruction(&Instruction::I32Load(mem32(OFF_A)));
                f.instruction(&Instruction::Call(self.fn_stream_next));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                f.instruction(&Instruction::LocalSet(scope.rlen()));
                f.instruction(&Instruction::LocalSet(scope.rptr()));
                // The inner end is this stage's end too.
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Eqz);
                f.instruction(&Instruction::If(BlockType::Empty));
                self.emit_mark_done(&mut f);
                f.instruction(&Instruction::End);
                f.instruction(&Instruction::LocalGet(scope.rptr()));
                f.instruction(&Instruction::LocalGet(scope.rlen()));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
            }
            Stage::Host {
                read_fn,
                drop_stream_fn,
                drop_future_fn,
                third,
            } => self.emit_host_stage(
                *read_fn,
                *drop_stream_fn,
                *drop_future_fn,
                *third,
                &scope,
                &mut f,
            ),
            Stage::Map { param, body, .. } => {
                f.instruction(&Instruction::LocalGet(0));
                f.instruction(&Instruction::I32Load(mem32(OFF_A)));
                f.instruction(&Instruction::Call(self.fn_stream_next));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr() + 1));
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr()));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Eqz);
                f.instruction(&Instruction::If(BlockType::Empty));
                emit_end_return(&mut f);
                f.instruction(&Instruction::End);
                let elem_repr = match self.resolve_repr(param) {
                    Ty::NamedStr(n) => Ty::NamedStr(n),
                    _ => Ty::Str,
                };
                let inner = self.lambda_elem_scope(param, &elem_repr, &scope);
                let saved = (self.cur_fn_early_return.take(), self.entry_fails);
                self.entry_fails = false;
                let out_ty = self.compile_block_return(body, &inner, &mut f);
                (self.cur_fn_early_return, self.entry_fails) = saved;
                if !out_ty.is_str_like() {
                    // Checker-rejected shape: keep the frame valid.
                    self.drop_value(out_ty, &mut f);
                    f.instruction(&Instruction::I32Const(0));
                    f.instruction(&Instruction::I32Const(0));
                }
                f.instruction(&Instruction::I32Const(1));
            }
        }
        f.instruction(&Instruction::End);
        f
    }

    /// Rewrite the stage's slot to `Done`.
    fn emit_mark_done(&self, f: &mut Function) {
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Store(mem32(OFF_SLOT)));
    }

    /// One host read into fresh room: `BLOCKED` (all ones, never from a
    /// sync read) and an empty read are the end; any code but
    /// `COMPLETED` ends the stream after this chunk. The bump pointer is
    /// reset to the chunk's end so the room past it is reused. At the
    /// end the handles are dropped and the slot rewritten to `Done`.
    fn emit_host_stage(
        &self,
        read_fn: u32,
        drop_stream_fn: u32,
        drop_future_fn: u32,
        third: Third,
        scope: &LocalScope,
        f: &mut Function,
    ) {
        let finish = |f: &mut Function| {
            self.emit_mark_done(f);
            f.instruction(&Instruction::LocalGet(0));
            f.instruction(&Instruction::I32Load(mem32(OFF_A)));
            f.instruction(&Instruction::Call(drop_stream_fn));
            f.instruction(&Instruction::LocalGet(0));
            f.instruction(&Instruction::I32Load(mem32(OFF_B)));
            f.instruction(&Instruction::Call(drop_future_fn));
            match third {
                Third::Nothing => {}
                Third::Descriptor { drop_fn } => {
                    f.instruction(&Instruction::LocalGet(0));
                    f.instruction(&Instruction::I32Load(mem32(OFF_C)));
                    f.instruction(&Instruction::Call(drop_fn));
                }
                Third::Settled {
                    write_fn,
                    drop_fn,
                    ok_at,
                } => {
                    f.instruction(&Instruction::LocalGet(0));
                    f.instruction(&Instruction::I32Load(mem32(OFF_C)));
                    f.instruction(&Instruction::I32Const(ok_at as i32));
                    f.instruction(&Instruction::Call(write_fn));
                    f.instruction(&Instruction::Drop);
                    f.instruction(&Instruction::LocalGet(0));
                    f.instruction(&Instruction::I32Load(mem32(OFF_C)));
                    f.instruction(&Instruction::Call(drop_fn));
                }
            }
        };
        f.instruction(&Instruction::I32Const(CHUNK));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32Load(mem32(OFF_A)));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(CHUNK));
        f.instruction(&Instruction::Call(read_fn));
        f.instruction(&Instruction::LocalTee(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(-1));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::If(BlockType::Empty));
        finish(f);
        emit_end_return(f);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(4));
        f.instruction(&Instruction::I32ShrU);
        f.instruction(&Instruction::LocalTee(scope.tmp_i32_b()));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::If(BlockType::Empty));
        finish(f);
        emit_end_return(f);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::GlobalSet(GLOBAL_BUMP_PTR));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(15));
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::If(BlockType::Empty));
        finish(f);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::I32Const(1));
    }

    /// Allocate a stage for `slot` with its cells taken from the given
    /// locals, leaving its pointer in `addr_scratch` and on the stack.
    fn emit_new_stage(
        &self,
        slot: u32,
        cells: [Option<u32>; 3],
        scope: &LocalScope,
        f: &mut Function,
    ) {
        f.instruction(&Instruction::I32Const(STAGE_SIZE));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(slot as i32));
        f.instruction(&Instruction::I32Store(mem32(OFF_SLOT)));
        for (cell, offset) in cells.iter().zip([OFF_A, OFF_B, OFF_C]) {
            f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
            match cell {
                Some(local) => f.instruction(&Instruction::LocalGet(*local)),
                None => f.instruction(&Instruction::I32Const(0)),
            };
            f.instruction(&Instruction::I32Store(mem32(offset)));
        }
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
    }

    /// A `Host` stage over the handles in the given locals — the
    /// stream, its completion future, and the third cell's when it has
    /// one. Leaves the stage's pointer in `addr_scratch` and on the
    /// stack.
    pub(super) fn emit_host_stream(
        &mut self,
        stage: Stage,
        stream: u32,
        future: u32,
        third: Option<u32>,
        scope: &LocalScope,
        f: &mut Function,
    ) {
        let slot = self.stream_stage(stage);
        self.emit_new_stage(slot, [Some(stream), Some(future), third], scope, f);
    }

    /// `stream -> First`: the next chunk as `Option<String>`. The stage
    /// is on the stack.
    pub(super) fn compile_stream_next(&mut self, scope: &LocalScope, f: &mut Function) -> Ty {
        f.instruction(&Instruction::Call(self.fn_stream_next));
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        f.instruction(&Instruction::LocalSet(scope.rlen()));
        f.instruction(&Instruction::LocalSet(scope.rptr()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        self.build_option_some(Ty::Str, scope, f);
        f.instruction(&Instruction::Else);
        f.instruction(&Instruction::I32Const(12));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalTee(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Store(mem32(0)));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::End);
        Ty::NamedPtrOf(
            "Option".to_string(),
            "String".to_string(),
            "String".to_string(),
        )
    }

    /// `stream -> Folded(init * (Acc * Chunk) => Acc { … })`, pulling
    /// chunk by chunk: the accumulator lives in the fold locals, the
    /// chunk in the map-element pair, and the stage rides the operand
    /// stack in the list loops' trio block where the body cannot touch
    /// it. The stage is on the stack under the fold's arguments.
    pub(super) fn compile_stream_fold(
        &mut self,
        init: &Expr,
        acc_name: &str,
        elem_name: &str,
        body: &Block,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let trio = self.list_loop_trio_ty();
        let acc_repr = self.resolve_repr(acc_name);
        let elem_repr = match self.resolve_repr(elem_name) {
            Ty::NamedStr(n) => Ty::NamedStr(n),
            _ => Ty::Str,
        };
        let init_ty = self.compile_expr(init, scope, f);
        self.store_fold_acc(&init_ty, scope, f);
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::Block(BlockType::FunctionType(trio)));
        f.instruction(&Instruction::Loop(BlockType::FunctionType(trio)));
        // [stage, 0, 0] → pull; the end leaves the block with the trio.
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::LocalTee(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::Call(self.fn_stream_next));
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        f.instruction(&Instruction::LocalSet(scope.map_elem_ptr() + 1));
        f.instruction(&Instruction::LocalSet(scope.map_elem_ptr()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::BrIf(1));
        let mut inner = self.lambda_elem_scope(elem_name, &elem_repr, scope);
        let acc_local = match acc_repr {
            Ty::I64 => scope.fold_acc_i64(),
            Ty::F64 => scope.fold_acc_f64(),
            _ => scope.fold_acc_ptr(),
        };
        inner
            .vars
            .insert(acc_name.to_string(), (acc_local, acc_repr.clone()));
        let out_ty = self.compile_block_return(body, &inner, f);
        self.store_fold_acc(&out_ty, scope, f);
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::Drop);
        self.push_local(acc_local, &acc_repr, f);
        acc_repr
    }

    /// `stream -> Mapped((Chunk) => Line { … })`: a `Map` stage over the
    /// stage on the stack; the lambda compiles into the stage function.
    pub(super) fn compile_stream_map(
        &mut self,
        param: &str,
        body: &Block,
        site: crate::error::Span,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let slot = self.stream_stage(Stage::Map {
            param: param.to_string(),
            body: body.clone(),
            site,
        });
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
        self.emit_new_stage(slot, [Some(scope.tmp_i32()), None, None], scope, f);
        Ty::NamedPtr("Stream".to_string())
    }

    /// `stream -> Taken(n)`: a `Taken` stage over the stage on the stack.
    pub(super) fn compile_stream_take(
        &mut self,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let slot = self.stream_stage(Stage::Taken);
        // The count is user code: compile it before touching scratch.
        self.compile_i64_arg(args, scope, f);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
        self.emit_new_stage(
            slot,
            [Some(scope.tmp_i32()), Some(scope.tmp_i32_b()), None],
            scope,
            f,
        );
        Ty::NamedPtr("Stream".to_string())
    }

    /// `list -> Stream`: a `List` stage over the `(ptr, len)` on the
    /// stack.
    pub(super) fn compile_list_to_stream(&mut self, scope: &LocalScope, f: &mut Function) -> Ty {
        let slot = self.stream_stage(Stage::List);
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
        self.emit_new_stage(
            slot,
            [Some(scope.tmp_i32()), Some(scope.tmp_i32_b()), None],
            scope,
            f,
        );
        Ty::NamedPtr("Stream".to_string())
    }
}
