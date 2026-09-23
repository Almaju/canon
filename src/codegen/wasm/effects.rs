//! Effects: file and HTTP I/O, stream drains and writes, async subtasks, and the `Parallel` / `Race` combinators.
use super::*;

impl<'m> WasmGen<'m> {
    /// Read the stream in `stream` to its end, chunk after chunk, at
    /// `pos` onward — `pos` ends at the byte past the last one read.
    /// Chunks land back to back: the bump pointer is reset to the
    /// running position before each further reserve, so the next
    /// chunk's room starts exactly there (the allocator's own 8-byte
    /// rounding only moves where a *later* value goes), and is left at
    /// the end. The caller reserves the first chunk's room past `pos`.
    /// `status` holds the packed read status `(count << 4) | code`;
    /// `BLOCKED` (all ones) never comes back from a sync read and is
    /// treated as the end, as is any code but `COMPLETED` or an empty
    /// read.
    pub(super) fn emit_drain_stream(
        &mut self,
        read_fn: u32,
        stream: u32,
        pos: u32,
        status: u32,
        f: &mut Function,
    ) {
        const CHUNK: i32 = 65536;
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(stream));
        f.instruction(&Instruction::LocalGet(pos));
        f.instruction(&Instruction::I32Const(CHUNK));
        f.instruction(&Instruction::Call(read_fn));
        f.instruction(&Instruction::LocalTee(status));
        f.instruction(&Instruction::I32Const(-1));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(pos));
        f.instruction(&Instruction::LocalGet(status));
        f.instruction(&Instruction::I32Const(4));
        f.instruction(&Instruction::I32ShrU);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(pos));
        f.instruction(&Instruction::LocalGet(status));
        f.instruction(&Instruction::I32Const(15));
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(status));
        f.instruction(&Instruction::I32Const(4));
        f.instruction(&Instruction::I32ShrU);
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(pos));
        f.instruction(&Instruction::GlobalSet(GLOBAL_BUMP_PTR));
        f.instruction(&Instruction::I32Const(CHUNK + 8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(pos));
        f.instruction(&Instruction::GlobalSet(GLOBAL_BUMP_PTR));
    }

    /// Open the file at the path in `(path, path + 1)`: an absolute
    /// path under the preopen named `/`, a relative one under the
    /// preopen named `.` — the first preopen when neither is offered —
    /// with the given `open-flags` and `descriptor-flags`
    /// (`symlink-follow` always), awaiting the async `open-at`. Its six
    /// flat params travel through memory (more than the async lower
    /// passes flat); its `result<descriptor, error-code>` lands in
    /// `alloc_ptr`: on `Ok` the descriptor is left in `tmp_i32`; on
    /// `Err` the case is at +4 and, for `other`, the message at +8
    /// (disc) / +12 / +16. Every preopen handle is dropped again. Uses
    /// `tmp_i32`, `tmp_i32_b`, `rbool`, `rptr`, `rlen`, `addr_scratch`,
    /// `par_seen_b`, `par_set`, `par_event_ptr`.
    pub(super) fn emit_open_at(
        &mut self,
        base: u32,
        open_flags: i32,
        descriptor_flags: i32,
        path: u32,
        scope: &LocalScope,
        f: &mut Function,
    ) {
        let imp = |i: FileImport| base + 1 + i as u32;
        let mem32 = |offset: u64| MemArg {
            offset,
            align: 2,
            memory_index: 0,
        };
        let mem8 = |offset: u64| MemArg {
            offset,
            align: 0,
            memory_index: 0,
        };
        // Absolute: the wanted preopen name is `/` and the path loses
        // its first byte; relative: `.`.
        f.instruction(&Instruction::LocalGet(path));
        f.instruction(&Instruction::I32Load8U(mem8(0)));
        f.instruction(&Instruction::I32Const(b'/' as i32));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::LocalGet(path + 1));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        f.instruction(&Instruction::LocalGet(path));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(path));
        f.instruction(&Instruction::LocalGet(path + 1));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(path + 1));
        f.instruction(&Instruction::I32Const(b'/' as i32));
        f.instruction(&Instruction::I32Const(b'.' as i32));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::Select);
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        // get-directories → (ptr, len) of 12-byte entries: a handle,
        // then the name. Walk them: the match (or the first) is kept
        // in par_seen_b, every other handle dropped.
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalTee(scope.addr_scratch()));
        f.instruction(&Instruction::Call(imp(FileImport::GetDirectories)));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Load(mem32(0)));
        f.instruction(&Instruction::LocalSet(scope.rptr()));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Load(mem32(4)));
        f.instruction(&Instruction::LocalSet(scope.rlen()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.par_seen_b()));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::I32Load(mem32(0)));
        f.instruction(&Instruction::LocalSet(scope.par_set()));
        // Wanted: a one-byte name equal to the wanted byte, and nothing
        // kept yet or a first-entry placeholder.
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::I32Load(mem32(8)));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::I32Load(mem32(4)));
        f.instruction(&Instruction::I32Load8U(mem8(0)));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::I32Or);
        f.instruction(&Instruction::If(BlockType::Empty));
        // Keep this one; a placeholder kept earlier is dropped.
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::Call(imp(FileImport::DescriptorDrop)));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::LocalSet(scope.par_seen_b()));
        f.instruction(&Instruction::Else);
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(imp(FileImport::DescriptorDrop)));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::I32Const(12));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.rptr()));
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(scope.rlen()));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        // open-at(root, symlink-follow, path, open-flags, flags) → the
        // params area (each `flags` a byte), then the ret area.
        f.instruction(&Instruction::I32Const(24));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::I32Store(mem32(0)));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Store(mem32(4)));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(path));
        f.instruction(&Instruction::I32Store(mem32(8)));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(path + 1));
        f.instruction(&Instruction::I32Store(mem32(12)));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(open_flags));
        f.instruction(&Instruction::I32Store8(mem8(16)));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(descriptor_flags));
        f.instruction(&Instruction::I32Store8(mem8(17)));
        f.instruction(&Instruction::I32Const(24));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::Call(imp(FileImport::OpenAt)));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
        self.emit_subtask_wait(scope, f);
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::Call(imp(FileImport::DescriptorDrop)));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Load(mem32(4)));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
    }

    /// With `open-at`'s result in `alloc_ptr` and its `Err` taken: the
    /// error text, `"NN"` (the case number) and, for `other`, a space
    /// and its message — ptr in `addr_scratch`, len in `rlen`. Uses
    /// `rbool`, `rptr`, `tmp_i32_b`.
    pub(super) fn emit_file_error(&mut self, scope: &LocalScope, f: &mut Function) {
        let mem32 = |offset: u64| MemArg {
            offset,
            align: 2,
            memory_index: 0,
        };
        let mem8 = |offset: u64| MemArg {
            offset,
            align: 0,
            memory_index: 0,
        };
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Load8U(mem8(4)));
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        // The message: src in tmp_i32_b, n in rlen (0 without one).
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.rlen()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Const(36));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Load8U(mem8(8)));
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Load(mem32(12)));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Load(mem32(16)));
        f.instruction(&Instruction::LocalSet(scope.rlen()));
        f.instruction(&Instruction::End);
        // "NN" then " message".
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        f.instruction(&Instruction::I32Const(3));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Const(10));
        f.instruction(&Instruction::I32DivU);
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(mem8(0)));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Const(10));
        f.instruction(&Instruction::I32RemU);
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(mem8(1)));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(b' ' as i32));
        f.instruction(&Instruction::I32Store8(mem8(2)));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(3));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.rptr()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        self.emit_byte_copy_loop(scope, f);
        // Without a message the text is just the two digits.
        f.instruction(&Instruction::I32Const(2));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::I32Const(3));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::Select);
        f.instruction(&Instruction::LocalSet(scope.rlen()));
    }

    /// The `Result` struct: tag 1 (Ok) or 0 (Err) from `tag`, then the
    /// string in `addr_scratch` / `rlen`.
    pub(super) fn emit_result_string(
        &mut self,
        tag: u32,
        ok_name: String,
        err_name: String,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let mem32 = |offset: u64| MemArg {
            offset,
            align: 2,
            memory_index: 0,
        };
        f.instruction(&Instruction::I32Const(12));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(tag));
        f.instruction(&Instruction::I32Store(mem32(0)));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Store(mem32(4)));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        f.instruction(&Instruction::I32Store(mem32(8)));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        Ty::NamedPtrOf("Result".to_string(), ok_name, err_name)
    }

    /// The fused file read — see `IndirectReturnShape::FileRead`. On
    /// entry the path sits on the stack; no user code runs from here
    /// on.
    pub(super) fn emit_file_read(
        &mut self,
        read_fn: u32,
        ok_name: String,
        err_name: String,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let imp = |i: FileImport| read_fn + 1 + i as u32;
        let mem32 = |offset: u64| MemArg {
            offset,
            align: 2,
            memory_index: 0,
        };
        let mem8 = |offset: u64| MemArg {
            offset,
            align: 0,
            memory_index: 0,
        };
        let path = scope.arm_payload_ptr();
        f.instruction(&Instruction::LocalSet(path + 1));
        f.instruction(&Instruction::LocalSet(path));
        self.emit_open_at(read_fn, 0, 1, path, scope, f);
        // The tag: 1 on Ok.
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Load8U(mem8(0)));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));
        f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
        f.instruction(&Instruction::If(BlockType::Empty));
        // read-via-stream(descriptor, offset 0, ret) → stream at +0,
        // completion future at +4; drain from a fresh start.
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.par_set()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(read_fn));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::I32Load(mem32(0)));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::I32Load(mem32(4)));
        f.instruction(&Instruction::LocalSet(scope.par_event_ptr()));
        // The `Ok` payload is a `Host` stage over the stream, which
        // drops the descriptor with the handles at its end. The
        // stage's pointer takes the string's place in `addr_scratch`,
        // with a zero length beside it.
        self.emit_host_stream(
            stream::Stage::Host {
                read_fn: imp(FileImport::BodyRead),
                drop_stream_fn: imp(FileImport::BodyDropReadable),
                drop_future_fn: imp(FileImport::BodyTrailersDropReadable),
                third: stream::Third::Descriptor {
                    drop_fn: imp(FileImport::DescriptorDrop),
                },
            },
            scope.tmp_i32_b(),
            scope.par_event_ptr(),
            Some(scope.tmp_i32()),
            scope,
            f,
        );
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.rlen()));
        f.instruction(&Instruction::Else);
        self.emit_file_error(scope, f);
        f.instruction(&Instruction::End);
        self.emit_result_string(scope.par_seen_a(), ok_name, err_name, scope, f)
    }

    /// The pumped stream write — see `IndirectReturnShape::StreamWrite`.
    /// On entry the stream's stage sits on the stack; no user code runs
    /// from here on. The completion is read once the writer is dropped:
    /// a value that is `err` names its `error-code` case (the `Err`
    /// string), anything else is `Ok`.
    pub(super) fn emit_stream_write(
        &mut self,
        ok_name: String,
        err_name: String,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let mem32 = |offset: u64| MemArg {
            offset,
            align: 2,
            memory_index: 0,
        };
        let mem8 = |offset: u64| MemArg {
            offset,
            align: 0,
            memory_index: 0,
        };
        let (stage, reader, writer, future) = (
            scope.par_subtask_a(),
            scope.par_retarea_a(),
            scope.par_retarea_b(),
            scope.par_set(),
        );
        f.instruction(&Instruction::LocalSet(stage));
        f.instruction(&Instruction::Call(FN_STDOUT_STREAM_NEW));
        f.instruction(&Instruction::LocalTee(scope.tmp_i64()));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(reader));
        f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
        f.instruction(&Instruction::I64Const(32));
        f.instruction(&Instruction::I64ShrU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(writer));
        f.instruction(&Instruction::LocalGet(reader));
        f.instruction(&Instruction::Call(FN_STDOUT_WRITE_VIA_STREAM));
        f.instruction(&Instruction::LocalSet(future));
        // Pull and write until the stream ends; the write status is
        // dropped, as `print_str` drops it.
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(stage));
        f.instruction(&Instruction::Call(self.fn_stream_next));
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        f.instruction(&Instruction::LocalSet(scope.rlen()));
        f.instruction(&Instruction::LocalSet(scope.rptr()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(writer));
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        f.instruction(&Instruction::Call(FN_STDOUT_STREAM_WRITE));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(writer));
        f.instruction(&Instruction::Call(FN_STDOUT_STREAM_DROP_WRITABLE));
        // The completion: `(count << 4) | code`, the value at
        // `addr_scratch` when the count is one — disc byte, then the
        // `error-code` case.
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(future));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::Call(FN_STDOUT_FUTURE_READ));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(future));
        f.instruction(&Instruction::Call(FN_STDOUT_FUTURE_DROP_READABLE));
        // The `Result`: tag 0 (Err) only for an `err` value that arrived.
        f.instruction(&Instruction::I32Const(12));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(4));
        f.instruction(&Instruction::I32ShrU);
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Load8U(mem8(0)));
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::I32Store(mem32(0)));
        // The case name, in the WIT's declaration order
        // (packages/canon/wit/wasi/cli.wit), stored whether or not it
        // is read.
        let cases: Vec<(u32, u32)> = ["io", "illegal-byte-sequence", "pipe"]
            .iter()
            .map(|name| self.strings.intern(name))
            .collect();
        for (offset, pick) in [(4u64, 0usize), (8, 1)] {
            let part = |case: usize| [cases[case].0, cases[case].1][pick] as i32;
            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
            f.instruction(&Instruction::I32Const(part(0)));
            f.instruction(&Instruction::I32Const(part(1)));
            f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
            f.instruction(&Instruction::I32Load8U(mem8(1)));
            f.instruction(&Instruction::I32Eqz);
            f.instruction(&Instruction::Select);
            f.instruction(&Instruction::I32Const(part(2)));
            f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
            f.instruction(&Instruction::I32Load8U(mem8(1)));
            f.instruction(&Instruction::I32Const(2));
            f.instruction(&Instruction::I32LtU);
            f.instruction(&Instruction::Select);
            f.instruction(&Instruction::I32Store(mem32(offset)));
        }
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        Ty::NamedPtrOf("Result".to_string(), ok_name, err_name)
    }

    /// The fused file write — see `IndirectReturnShape::FileWrite`. On
    /// entry the contents then the path sit on the stack; no user code
    /// runs from here on.
    pub(super) fn emit_file_write(
        &mut self,
        write_fn: u32,
        ok_name: String,
        err_name: String,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let imp = |i: FileWriteImport| write_fn + 1 + i as u32;
        let mem8 = |offset: u64| MemArg {
            offset,
            align: 0,
            memory_index: 0,
        };
        let path = scope.arm_payload_ptr();
        let contents = scope.fold_acc_ptr();
        // The path as written, for the `Ok` arm: opening trims it.
        let written = scope.str_scratch_ptr();
        f.instruction(&Instruction::LocalSet(path + 1));
        f.instruction(&Instruction::LocalSet(path));
        f.instruction(&Instruction::LocalGet(path));
        f.instruction(&Instruction::LocalSet(written));
        f.instruction(&Instruction::LocalGet(path + 1));
        f.instruction(&Instruction::LocalSet(written + 1));
        f.instruction(&Instruction::LocalSet(contents + 1));
        f.instruction(&Instruction::LocalSet(contents));
        // create | truncate; write.
        self.emit_open_at(write_fn, 1 | 8, 2, path, scope, f);
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Load8U(mem8(0)));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));
        f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
        f.instruction(&Instruction::If(BlockType::Empty));
        // A fresh stream: its reader goes to write-via-stream, the
        // contents through its writer, then the completion future is
        // read for the outcome.
        f.instruction(&Instruction::Call(imp(FileWriteImport::ContentsNew)));
        f.instruction(&Instruction::LocalTee(scope.tmp_i64()));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(scope.par_set()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
        f.instruction(&Instruction::I64Const(32));
        f.instruction(&Instruction::I64ShrU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::Call(write_fn));
        f.instruction(&Instruction::LocalSet(scope.par_event_ptr()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::LocalGet(contents));
        f.instruction(&Instruction::LocalGet(contents + 1));
        f.instruction(&Instruction::Call(imp(FileWriteImport::ContentsWrite)));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::Call(imp(
            FileWriteImport::ContentsDropWritable,
        )));
        // The outcome lands where open-at's did, with the same layout
        // for an error.
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::Call(imp(FileWriteImport::DoneRead)));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::Call(imp(FileWriteImport::DoneDropReadable)));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::Call(
            write_fn + 1 + FileImport::DescriptorDrop as u32,
        ));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Load8U(mem8(0)));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
        f.instruction(&Instruction::If(BlockType::Empty));
        // Ok carries the path back.
        f.instruction(&Instruction::LocalGet(written));
        f.instruction(&Instruction::LocalSet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(written + 1));
        f.instruction(&Instruction::LocalSet(scope.rlen()));
        f.instruction(&Instruction::Else);
        self.emit_file_error(scope, f);
        f.instruction(&Instruction::End);
        self.emit_result_string(scope.par_seen_a(), ok_name, err_name, scope, f)
    }

    /// With a packed subtask status in `tmp_i32`, block until the
    /// subtask has returned: when the low 4 bits say it has not, join
    /// its handle (the high 28 bits) to a fresh waitable set, wait on
    /// the set, and drop both.
    pub(super) fn emit_subtask_wait(&mut self, scope: &LocalScope, f: &mut Function) {
        // Check `status & 0xF != 2` (i.e. *not* `Returned`).
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(0xF));
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::I32Const(2));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::If(BlockType::Empty));
        // ── Async-suspend path ─────────────────────────────────────────
        // The subtask has been started but not yet finished. Extract its
        // handle (high 28 bits of the packed status), wrap it in a
        // single-element waitable-set, and block on `waitable-set.wait`.
        // The host signals subtask completion through the waitable; when
        // wait returns, the result has been written to our ret-area.
        //
        // We re-use scratch locals from the surrounding function's
        // extra-locals pool:
        //   tmp_i32_b → subtask handle
        //   rbool     → waitable-set handle
        //   rptr      → event-area pointer (8 bytes, written by wait)
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(4));
        f.instruction(&Instruction::I32ShrU);
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        // set = waitable-set.new()
        f.instruction(&Instruction::Call(self.fn_waitable_set_new));
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        // waitable.join(subtask, set)
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::Call(self.fn_waitable_join));
        // event_area = $alloc(8) — wait writes the 8-byte event payload here.
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.rptr()));
        // waitable-set.wait(set, event_area) delivers the subtask's
        // next event — `(subtask, state)` in the event area — and a
        // subtask reports `STARTED` before `RETURNED`, so wait again
        // until the state is terminal (2). The event code is dropped:
        // the only thing in the set is our subtask.
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_wait));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 4,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::I32Const(2));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::BrIf(0));
        f.instruction(&Instruction::End);
        // Drop the subtask BEFORE the waitable-set: the subtask is
        // joined to the set as a child, so dropping the set while the
        // subtask is still registered trips wasmtime's
        // `ResourceTableError::HasChildren` check (see
        // `wasmtime::runtime::component::concurrent::waitable_set_drop`).
        // Dropping the subtask removes it from the set's child list;
        // the set then drops cleanly.
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::Call(self.fn_subtask_drop));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_drop));
        f.instruction(&Instruction::End);
    }

    /// The fused `wasi:http/client` round trip — see
    /// `IndirectReturnShape::HttpSend`. On entry the six strings sit on
    /// the stack in declaration order (`Authority * Body * Method *
    /// PathWithQuery * RequestHeaders * Scheme`); no user code runs
    /// from here on, so the scratch locals are free:
    ///
    ///   1. `fields` from the `name: value` lines of the headers,
    ///   2. a trailers future, a contents stream when the body is
    ///      non-empty, then `request.new` and the method / scheme /
    ///      authority / path setters — every one spelled through its
    ///      `other(string)` case, which the host normalises,
    ///   3. `[async-lower]send`; the body is written (a sync write
    ///      blocks until the host has read it) and the trailers future
    ///      resolved to `ok(none)` while the request is in flight, then
    ///      the subtask is awaited,
    ///   4. `ok(response)`: the status code becomes the `"NNN "` prefix
    ///      and `consume-body`'s stream drains behind it, as
    ///      `ByteStream` drains; `err(error-code)`: `"0NN "` with the
    ///      case number, then the `internal-error` message when there
    ///      is one.
    ///
    /// The value is an ordinary Canon `Result` struct with the string in
    /// its `Ok` arm; the stdlib reads the prefix.
    pub(super) fn emit_http_send(
        &mut self,
        send_fn: u32,
        ok_name: String,
        err_name: String,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        const CHUNK: i32 = 65536;
        let imp = |i: HttpSendImport| send_fn + 1 + i as u32;
        let mem32 = |offset: u64| MemArg {
            offset,
            align: 2,
            memory_index: 0,
        };
        let mem8 = |offset: u64| MemArg {
            offset,
            align: 0,
            memory_index: 0,
        };
        // The inputs, top of stack first.
        let scheme = scope.str_scratch_ptr();
        let headers = scope.lit_scrut_ptr();
        let path = scope.map_elem_ptr();
        let method = scope.bind_scrut_ptr();
        let body = scope.fold_acc_ptr();
        let authority = scope.arm_payload_ptr();
        for base in [scheme, headers, path, method, body, authority] {
            f.instruction(&Instruction::LocalSet(base + 1));
            f.instruction(&Instruction::LocalSet(base));
        }

        // ── 1. fields ────────────────────────────────────────────────
        f.instruction(&Instruction::Call(imp(HttpSendImport::FieldsNew)));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
        // Cursor `i` (par_seen_b) over the header bytes; each line
        // `[start, i)` (start = par_seen_a) splits at its first `:`
        // (par_set, -1 when none) into a name and a value that skips
        // the spaces after the colon (par_event_ptr = value start).
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.par_seen_b()));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::LocalGet(headers + 1));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));
        f.instruction(&Instruction::I32Const(-1));
        f.instruction(&Instruction::LocalSet(scope.par_set()));
        // Scan to the end of the line.
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::LocalGet(headers + 1));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(headers));
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load8U(mem8(0)));
        f.instruction(&Instruction::LocalTee(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Const(b'\n' as i32));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::BrIf(1));
        // First colon of the line.
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Const(b':' as i32));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32LtS);
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::LocalSet(scope.par_set()));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.par_seen_b()));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        // A line with a colon is a header.
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32GeS);
        f.instruction(&Instruction::If(BlockType::Empty));
        // Value starts past the colon and any spaces.
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.par_event_ptr()));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(headers));
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load8U(mem8(0)));
        f.instruction(&Instruction::I32Const(b' ' as i32));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.par_event_ptr()));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        // fields.append(fields, name, value, ret)
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(headers));
        f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalGet(headers));
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::Call(imp(HttpSendImport::FieldsAppend)));
        f.instruction(&Instruction::End);
        // Past the newline.
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.par_seen_b()));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // ── 2. the request ───────────────────────────────────────────
        // Trailers future: reader (low 32) to request.new, writer
        // (high 32) for the post-send resolution.
        f.instruction(&Instruction::Call(imp(HttpSendImport::TrailersNew)));
        f.instruction(&Instruction::LocalTee(scope.tmp_i64()));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
        f.instruction(&Instruction::I64Const(32));
        f.instruction(&Instruction::I64ShrU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(scope.rptr()));
        // Contents stream only with a body: reader in par_event_ptr,
        // writer in rbool (0 without a body).
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        f.instruction(&Instruction::LocalGet(body + 1));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::Call(imp(HttpSendImport::ContentsNew)));
        f.instruction(&Instruction::LocalTee(scope.tmp_i64()));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(scope.par_event_ptr()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
        f.instruction(&Instruction::I64Const(32));
        f.instruction(&Instruction::I64ShrU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        f.instruction(&Instruction::End);
        // request.new(headers, contents?, trailers, options = none, ret)
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
        f.instruction(&Instruction::Call(imp(HttpSendImport::RequestNew)));
        f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
        f.instruction(&Instruction::I32Load(mem32(0)));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        // The transmission future is not consulted.
        f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
        f.instruction(&Instruction::I32Load(mem32(4)));
        f.instruction(&Instruction::Call(imp(
            HttpSendImport::TransmitDropReadable,
        )));
        // set-method(other(method)), set-scheme(some(other(scheme))),
        // set-authority(some), set-path-with-query(some); a rejected
        // value leaves the default, and the host reports it.
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::I32Const(9));
        f.instruction(&Instruction::LocalGet(method));
        f.instruction(&Instruction::LocalGet(method + 1));
        f.instruction(&Instruction::Call(imp(HttpSendImport::SetMethod)));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Const(2));
        f.instruction(&Instruction::LocalGet(scheme));
        f.instruction(&Instruction::LocalGet(scheme + 1));
        f.instruction(&Instruction::Call(imp(HttpSendImport::SetScheme)));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalGet(authority));
        f.instruction(&Instruction::LocalGet(authority + 1));
        f.instruction(&Instruction::Call(imp(HttpSendImport::SetAuthority)));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalGet(path));
        f.instruction(&Instruction::LocalGet(path + 1));
        f.instruction(&Instruction::Call(imp(HttpSendImport::SetPathWithQuery)));
        f.instruction(&Instruction::Drop);

        // ── 3. send ──────────────────────────────────────────────────
        f.instruction(&Instruction::I32Const(32));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::Call(send_fn));
        f.instruction(&Instruction::LocalSet(scope.rlen()));
        // The body, while the host reads it.
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::LocalGet(body));
        f.instruction(&Instruction::LocalGet(body + 1));
        f.instruction(&Instruction::Call(imp(HttpSendImport::ContentsWrite)));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::Call(imp(
            HttpSendImport::ContentsDropWritable,
        )));
        f.instruction(&Instruction::End);
        // Trailers: `ok(none)` is eight zero bytes.
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalTee(scope.par_seen_a()));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::I64Store(MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
        f.instruction(&Instruction::Call(imp(HttpSendImport::TrailersWrite)));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::Call(imp(
            HttpSendImport::TrailersDropWritable,
        )));
        // Await the response.
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
        self.emit_subtask_wait(scope, f);

        // ── 4. the answer: ptr in addr_scratch, len in rlen ──────────
        // `result<response, error-code>`: the discriminant byte at +0
        // and the payload at +8 (`error-code` holds an `option<u64>`
        // case, so it aligns to 8).
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Load8U(mem8(0)));
        f.instruction(&Instruction::If(BlockType::Empty));
        // err(error-code): case at +8; `internal-error` (39) carries
        // `option<string>` at +16 (disc) / +20 (ptr) / +24 (len).
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Load8U(mem8(8)));
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.rlen()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Const(39));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Load8U(mem8(16)));
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Load(mem32(20)));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Load(mem32(24)));
        f.instruction(&Instruction::LocalSet(scope.rlen()));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        f.instruction(&Instruction::I32Const(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.addr_scratch()));
        // "0NN "
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::I32Store8(mem8(0)));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Const(10));
        f.instruction(&Instruction::I32DivU);
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(mem8(1)));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Const(10));
        f.instruction(&Instruction::I32RemU);
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(mem8(2)));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(b' ' as i32));
        f.instruction(&Instruction::I32Store8(mem8(3)));
        // The message behind it: dst = start + 4, src, n.
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.rptr()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        self.emit_byte_copy_loop(scope, f);
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::I32Const(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.rlen()));
        f.instruction(&Instruction::Else);
        // ok(response): status code, then the body drained behind the
        // "NNN " prefix — chunks land back to back, the bump pointer
        // reset to the running position before each further reserve
        // (as `ByteStream` drains).
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Load(mem32(8)));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::Call(imp(HttpSendImport::GetStatusCode)));
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        f.instruction(&Instruction::I32Const(CHUNK + 8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Const(100));
        f.instruction(&Instruction::I32DivU);
        f.instruction(&Instruction::I32Const(10));
        f.instruction(&Instruction::I32RemU);
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(mem8(0)));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Const(10));
        f.instruction(&Instruction::I32DivU);
        f.instruction(&Instruction::I32Const(10));
        f.instruction(&Instruction::I32RemU);
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(mem8(1)));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Const(10));
        f.instruction(&Instruction::I32RemU);
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(mem8(2)));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(b' ' as i32));
        f.instruction(&Instruction::I32Store8(mem8(3)));
        // consume-body(response, res, ret); the `res` future is resolved
        // once the body is in hand.
        f.instruction(&Instruction::Call(imp(HttpSendImport::ResFutureNew)));
        f.instruction(&Instruction::LocalTee(scope.tmp_i64()));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
        f.instruction(&Instruction::I64Const(32));
        f.instruction(&Instruction::I64ShrU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(scope.par_seen_b()));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.par_set()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(imp(HttpSendImport::ConsumeBody)));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::I32Load(mem32(0)));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::I32Load(mem32(4)));
        f.instruction(&Instruction::LocalSet(scope.par_event_ptr()));
        // Drain behind the prefix: position in rptr, packed read status
        // in par_set.
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.rptr()));
        self.emit_drain_stream(
            imp(HttpSendImport::BodyRead),
            scope.tmp_i32_b(),
            scope.rptr(),
            scope.par_set(),
            f,
        );
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::Call(imp(HttpSendImport::BodyDropReadable)));
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::Call(imp(
            HttpSendImport::BodyTrailersDropReadable,
        )));
        // The `res` future resolves to `ok(_)` (eight zero bytes): the
        // body arrived, nothing to report.
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalTee(scope.par_set()));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::I64Store(MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(imp(HttpSendImport::ResFutureWrite)));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::Call(imp(
            HttpSendImport::ResFutureDropWritable,
        )));
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(scope.rlen()));
        f.instruction(&Instruction::End);

        // The `Result`: tag 1 (Ok), then the string's ptr and len.
        f.instruction(&Instruction::I32Const(12));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Store(mem32(0)));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Store(mem32(4)));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        f.instruction(&Instruction::I32Store(mem32(8)));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        Ty::NamedPtrOf("Result".to_string(), ok_name, err_name)
    }

    /// Emits the guest-side sequence for calling an `extern Wasm.async`
    /// function under the component-model async-lower ABI.
    ///
    /// At entry: args are already on the stack in their flat representation
    /// (just like a sync call), having been compiled by
    /// `emit_func_table_call` before the dispatch on `is_async`.
    ///
    /// Sequence:
    ///
    /// 1. **Ret-area** (only when the WIT-level function has a result).
    ///    Allocate `ret_area_size_for(&info.result_ty)` bytes via `$alloc`,
    ///    stash the pointer in `alloc_ptr`, and push it as the trailing
    ///    core-arg.
    /// 2. **Call** the async-lowered import. Its core signature is
    ///    `(flat_params …, ret_ptr?) -> i32` where the i32 result is a
    ///    *packed status word*:
    ///    - low 4 bits = `CallState` (0 Starting, 1 Started,
    ///      2 Returned, 3 StartCancelled, 4 ReturnCancelled)
    ///    - high 28 bits = subtask waitable handle (or 0 when Returned)
    /// 3. **Status check**. Save the status to `tmp_i32`, then mask the
    ///    low 4 bits and compare against `2 = Returned`. On the
    ///    sync-completion fast path we skip the wait block. Otherwise we
    ///    enter the **wait sequence**: extract the subtask handle from
    ///    the high 28 bits of the status, create a fresh waitable-set,
    ///    join the subtask into it, block on `waitable-set.wait`, and
    ///    drop both the set and the subtask after the wait returns. By
    ///    that point the host has written the actual result into our
    ///    ret-area.
    /// 4. **Decode result** from the ret-area according to
    ///    `info.result_ty`.
    pub(super) fn emit_async_call(
        &mut self,
        info: &FuncInfo,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let has_result = !matches!(info.result_ty, Ty::Unit);
        if has_result {
            // Allocate ret-area, save its ptr, and push it as the last arg.
            let size = ret_area_size_for(&info.result_ty);
            f.instruction(&Instruction::I32Const(size as i32));
            f.instruction(&Instruction::Call(self.fn_alloc));
            f.instruction(&Instruction::LocalTee(scope.alloc_ptr()));
        }
        // Call the async-lowered import. Stack on return: i32 packed status.
        f.instruction(&Instruction::Call(info.func_idx));
        // Save the packed status so we can (a) check the low 4 bits and
        // (b) recover the subtask handle from the high 28 bits if we
        // need to wait.
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
        self.emit_subtask_wait(scope, f);
        // Read the result out of the ret-area (still in `alloc_ptr`).
        if !has_result {
            return Ty::Unit;
        }
        match &info.result_ty {
            Ty::Str | Ty::NamedStr(_) => {
                // String result: (ptr i32 at +0, len i32 at +4).
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                info.result_ty.clone()
            }
            Ty::I64 | Ty::F64 => {
                // 8-byte scalar at +0.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                if matches!(info.result_ty, Ty::I64) {
                    f.instruction(&Instruction::I64Load(MemArg {
                        offset: 0,
                        align: 3,
                        memory_index: 0,
                    }));
                } else {
                    f.instruction(&Instruction::F64Load(MemArg {
                        offset: 0,
                        align: 3,
                        memory_index: 0,
                    }));
                }
                info.result_ty.clone()
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) => {
                // 4-byte scalar / handle at +0.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                info.result_ty.clone()
            }
            // List / NamedPtrOf / Unit fall here. The current codegen
            // doesn't synthesise async externs returning these shapes —
            // they'd need their own ret-area decoders. Trap so the gap is
            // visible if we ever do.
            _ => {
                f.instruction(&Instruction::Unreachable);
                info.result_ty.clone()
            }
        }
    }

    // ── Concurrency combinators ─────────────────────────────────────
    //
    // `parallel(a, b)` and `race(a, b)` are guest-side combinators: the
    // codegen emits a non-blocking async call for each arg (capturing
    // subtask handle + ret-area into named locals), then runs the
    // canonical-ABI multi-subtask wait sequence in the same function.
    // No host bridge is involved — the `$root` canon intrinsics
    // (`waitable-set-new`, `waitable-join`, `waitable-set-wait`,
    // `waitable-set-drop`, `subtask-drop`, `subtask-cancel`) handle
    // everything.

    /// Compile a single `parallel`/`race` argument as a non-blocking
    /// async call. The arg must be a `MethodCall` or `Constructor` that
    /// resolves to an `extern Wasm.async` function in `func_table`.
    ///
    /// On exit:
    ///   - The arg's sub-args are evaluated.
    ///   - The arg's ret-area is allocated into `retarea_local`.
    ///   - The import is called; the packed status is consumed.
    ///   - The subtask handle (status >> 4) is stored in `subtask_local`.
    ///
    /// Returns the callee's declared `result_ty` so the caller knows how
    /// to decode the ret-area later.
    ///
    /// Today this is conservative: if the arg shape doesn't match a known
    /// async extern, the codegen traps via `unreachable`. The checker
    /// can't surface a friendlier error yet because the surface is brand
    /// new; clean up once user pain reports.
    pub(super) fn emit_arg_as_nonblocking(
        &mut self,
        arg: &Expr,
        scope: &LocalScope,
        f: &mut Function,
        subtask_local: u32,
        retarea_local: u32,
    ) -> Ty {
        // Resolve the callee FuncInfo and identify the receiver / args.
        let resolved: Option<(FuncInfo, Option<Box<Expr>>, Vec<Expr>)> = match arg {
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => {
                let recv_ty_name = self.infer_static_type_name(receiver);
                let key = recv_ty_name.map(|n| (Some(n), method.name.clone()));
                let info = key
                    .and_then(|k| self.func_table.get(&k).cloned())
                    .or_else(|| self.func_table.get(&(None, method.name.clone())).cloned());
                info.map(|i| (i, Some(receiver.clone()), args.clone()))
            }
            Expr::Constructor { name, args, .. } => {
                // Try free-function key first.
                let mut info = self.func_table.get(&(None, name.name.clone())).cloned();
                // Then try Self-renamed constructor.
                if info.is_none() {
                    info = self
                        .func_table
                        .get(&(Some(name.name.clone()), "Self".to_string()))
                        .cloned();
                }
                // Then try capability-receiver: first arg's type as receiver.
                if info.is_none() {
                    if let Some(first) = args.first() {
                        if let Some(tname) = self.infer_static_type_name(first) {
                            info = self
                                .func_table
                                .get(&(Some(tname), name.name.clone()))
                                .cloned();
                        }
                    }
                }
                info.map(|i| (i, None, args.clone()))
            }
            _ => None,
        };

        let Some((info, receiver_opt, args_to_push)) = resolved else {
            // Couldn't resolve the call; trap. Callers should ensure the
            // arg points to a real async extern.
            f.instruction(&Instruction::Unreachable);
            return Ty::Unit;
        };

        if !info.is_async {
            // Only async calls make sense here — a sync call would
            // complete immediately and there'd be no subtask to wait on.
            f.instruction(&Instruction::Unreachable);
            return info.result_ty.clone();
        }

        // Push the receiver expression first (for MethodCall form). The
        // receiver becomes the first param of the import call.
        if let Some(rcv) = receiver_opt {
            let _ = self.compile_expr(&rcv, scope, f);
        }
        // Then the explicit args.
        for a in args_to_push {
            let _ = self.compile_expr(&a, scope, f);
        }

        // Allocate the ret-area and tee into `retarea_local` (leaving the
        // ptr on the stack as the last param to the import).
        let has_result = !matches!(info.result_ty, Ty::Unit);
        if has_result {
            let size = ret_area_size_for(&info.result_ty);
            f.instruction(&Instruction::I32Const(size as i32));
            f.instruction(&Instruction::Call(self.fn_alloc));
            f.instruction(&Instruction::LocalTee(retarea_local));
        } else {
            f.instruction(&Instruction::I32Const(0));
            f.instruction(&Instruction::LocalSet(retarea_local));
        }

        // Call the async-lowered import. Stack on return: i32 packed status.
        f.instruction(&Instruction::Call(info.func_idx));

        // Extract subtask handle = status >> 4. The low 4 bits encode the
        // CallState; the high 28 bits are the subtask waitable handle.
        f.instruction(&Instruction::I32Const(4));
        f.instruction(&Instruction::I32ShrU);
        f.instruction(&Instruction::LocalSet(subtask_local));

        info.result_ty.clone()
    }

    /// Emit `parallel(a, b)`: start both async calls non-blocking, join
    /// their subtasks to a fresh waitable-set, loop until both events
    /// fire, then build a `List<T>` with the two results in arg-order.
    ///
    /// Both args must call async externs returning the same payload type.
    /// The result type is `Ty::List`. Today only `Ty::Str` / `Ty::NamedStr`
    /// element shapes are decoded; other shapes trap.
    pub(super) fn compile_parallel(
        &mut self,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        if args.len() != 2 {
            // Surface error: parallel expects exactly two args. The
            // checker doesn't yet validate arity for synthetic combinators.
            f.instruction(&Instruction::Unreachable);
            return Ty::List;
        }

        // ── Start both calls non-blocking ─────────────────────────
        let ty_a = self.emit_arg_as_nonblocking(
            &args[0],
            scope,
            f,
            scope.par_subtask_a(),
            scope.par_retarea_a(),
        );
        let ty_b = self.emit_arg_as_nonblocking(
            &args[1],
            scope,
            f,
            scope.par_subtask_b(),
            scope.par_retarea_b(),
        );
        // Both arms must agree on element type.
        let _ = ty_b;
        let elem_ty = ty_a;

        // ── Build waitable-set, join both ──────────────────────────
        f.instruction(&Instruction::Call(self.fn_waitable_set_new));
        f.instruction(&Instruction::LocalSet(scope.par_set()));

        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_join));

        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_join));

        // ── Event area + seen flags ─────────────────────────────
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.par_event_ptr()));

        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.par_seen_b()));

        // ── Wait loop until both seen ───────────────────────────
        //
        // Structure:
        //   block $break
        //     loop $continue
        //       wait; drop event_code
        //       handle = load i32 at par_event_ptr+0
        //       handle == subtask_a ? seen_a = 1
        //       handle == subtask_b ? seen_b = 1
        //       (seen_a & seen_b) ? br $break (depth=1)
        //       br $continue (depth=0)
        //     end
        //   end
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));

        // waitable-set.wait(set, event_area) → event_code; drop event_code
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_wait));
        f.instruction(&Instruction::Drop);

        // event_handle = load i32 at par_event_ptr+0 → tmp_i32; the
        // state beside it must be `RETURNED` (2) — a subtask reports
        // `STARTED` first, and that event is not a result.
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 4,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::I32Const(2));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::BrIf(0));
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));

        // if event_handle == subtask_a: seen_a = 1
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));
        f.instruction(&Instruction::End);

        // if event_handle == subtask_b: seen_b = 1
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(scope.par_seen_b()));
        f.instruction(&Instruction::End);

        // if (seen_a & seen_b): br $break (depth 1 — the block above the loop)
        f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::BrIf(1));

        // br $continue (depth 0 — the loop itself)
        f.instruction(&Instruction::Br(0));

        f.instruction(&Instruction::End); // end loop
        f.instruction(&Instruction::End); // end block

        // ── Cleanup: drop subtasks before the set ────────────────────
        // Subtasks are children of the set; the set's drop requires no
        // children (see wasmtime's `ResourceTableError::HasChildren`).
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::Call(self.fn_subtask_drop));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::Call(self.fn_subtask_drop));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_drop));

        // ── Build List<T> with the two results ──────────────────────
        // List layout per `build_list_literal`: N*8 bytes, each slot is
        // (ptr i32, len i32) for Str / (8 bytes for I64/F64) at offsets
        // i*8. Total size = 16 for 2 elements.
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));

        match &elem_ty {
            // A `List<Unit>`: two empty slots — `alloc` hands out
            // zeroed memory.
            Ty::Unit => {}
            Ty::Str | Ty::NamedStr(_) => {
                // slot 0 ← (ptr,len) at par_retarea_a +0/+4
                self.copy_str_pair(f, scope.alloc_ptr(), 0, scope.par_retarea_a(), 0);
                // slot 1 ← (ptr,len) at par_retarea_b +0/+4
                self.copy_str_pair(f, scope.alloc_ptr(), 8, scope.par_retarea_b(), 0);
            }
            Ty::I64 | Ty::F64 => {
                // Each slot is one i64. Source ret-area holds the value at +0.
                for (slot_off, retarea) in
                    [(0u64, scope.par_retarea_a()), (8u64, scope.par_retarea_b())]
                {
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(retarea));
                    f.instruction(&Instruction::I64Load(MemArg {
                        offset: 0,
                        align: 3,
                        memory_index: 0,
                    }));
                    f.instruction(&Instruction::I64Store(MemArg {
                        offset: slot_off,
                        align: 3,
                        memory_index: 0,
                    }));
                }
            }
            _ => {
                // Other element shapes not yet supported. Trap so the gap
                // is visible (we'd silently corrupt the list otherwise).
                f.instruction(&Instruction::Unreachable);
            }
        }

        // Push (list_ptr, len=2) — the standard `Ty::List` representation.
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(2));
        Ty::List
    }

    /// Emit `race(a, b)`: start both async calls non-blocking, wait for
    /// the *first* event, cancel the loser, drop everything, and return
    /// the winner's result decoded from its ret-area.
    ///
    /// Today only string and unit results are decoded; other shapes
    /// trap.
    pub(super) fn compile_race(
        &mut self,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        if args.len() != 2 {
            f.instruction(&Instruction::Unreachable);
            return Ty::Str;
        }

        // Start both calls non-blocking.
        let ty_a = self.emit_arg_as_nonblocking(
            &args[0],
            scope,
            f,
            scope.par_subtask_a(),
            scope.par_retarea_a(),
        );
        let _ = self.emit_arg_as_nonblocking(
            &args[1],
            scope,
            f,
            scope.par_subtask_b(),
            scope.par_retarea_b(),
        );
        let elem_ty = ty_a;

        // Build waitable-set, join both.
        f.instruction(&Instruction::Call(self.fn_waitable_set_new));
        f.instruction(&Instruction::LocalSet(scope.par_set()));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_join));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_join));

        // Event area + flags. Re-using par_seen_a as "winner is a?".
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));

        // Wait until one subtask has returned (an earlier event only
        // says it started), then identify the winner.
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_wait));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 4,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::I32Const(2));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::BrIf(0));
        f.instruction(&Instruction::End);

        // Read event handle into tmp_i32, set seen_a = (handle == subtask_a).
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalTee(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));

        // Cancel the loser. `subtask.cancel` takes a subtask handle and
        // returns a state code (which we drop). The runtime guarantees
        // teardown of any transitive subtasks.
        //
        // The cancel call returns an i32 status code, even when issued
        // with async semantics. We drop it; the caller only cares that
        // the loser is no longer producing observable side effects.
        f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
        f.instruction(&Instruction::If(BlockType::Empty));
        // a won → cancel b
        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::Call(self.fn_subtask_cancel));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::Else);
        // b won → cancel a
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::Call(self.fn_subtask_cancel));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::End);

        // Drop both subtasks before the set.
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::Call(self.fn_subtask_drop));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::Call(self.fn_subtask_drop));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_drop));

        // Decode the winner's ret-area onto the stack.
        match &elem_ty {
            Ty::Str | Ty::NamedStr(_) => {
                // if seen_a: push (par_retarea_a +0, +4) else (par_retarea_b +0, +4)
                // WASM `if` with result type doesn't natively allow pushing two
                // values — use a Select-style approach via a winner_retarea local.
                // Compute winner_retarea via Select.
                f.instruction(&Instruction::LocalGet(scope.par_retarea_a()));
                f.instruction(&Instruction::LocalGet(scope.par_retarea_b()));
                f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
                f.instruction(&Instruction::Select);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));

                // Push ptr, then len.
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                elem_ty
            }
            Ty::Unit => elem_ty,
            _ => {
                f.instruction(&Instruction::Unreachable);
                elem_ty
            }
        }
    }

    /// Copy a `(ptr i32, len i32)` pair from `src_local + src_off` to
    /// `dst_local + dst_off`. Small helper used by the list-building tail
    /// of `compile_parallel`.
    pub(super) fn copy_str_pair(
        &self,
        f: &mut Function,
        dst_local: u32,
        dst_off: u64,
        src_local: u32,
        src_off: u64,
    ) {
        // ptr
        f.instruction(&Instruction::LocalGet(dst_local));
        f.instruction(&Instruction::LocalGet(src_local));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: src_off,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::I32Store(MemArg {
            offset: dst_off,
            align: 2,
            memory_index: 0,
        }));
        // len
        f.instruction(&Instruction::LocalGet(dst_local));
        f.instruction(&Instruction::LocalGet(src_local));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: src_off + 4,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::I32Store(MemArg {
            offset: dst_off + 4,
            align: 2,
            memory_index: 0,
        }));
    }
}
