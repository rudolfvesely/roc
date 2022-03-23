mod backend;
mod layout;
mod low_level;
mod storage;
pub mod wasm_module;

// Helpers for interfacing to a Wasm module from outside
pub mod wasm32_result;
pub mod wasm32_sized;

use bumpalo::{self, collections::Vec, Bump};

use roc_collections::all::{MutMap, MutSet};
use roc_module::low_level::LowLevelWrapperType;
use roc_module::symbol::{Interns, ModuleId, Symbol};
use roc_mono::code_gen_help::CodeGenHelp;
use roc_mono::ir::{Proc, ProcLayout};
use roc_mono::layout::LayoutIds;
use roc_target::TargetInfo;

use crate::backend::{ProcLookupData, ProcSource, WasmBackend};
use crate::wasm_module::{
    Align, CodeBuilder, Export, ExportType, LocalId, SymInfo, ValueType, WasmModule,
};

const TARGET_INFO: TargetInfo = TargetInfo::default_wasm32();
const PTR_SIZE: u32 = {
    let value = TARGET_INFO.ptr_width() as u32;

    // const assert that our pointer width is actually 4
    // the code relies on the pointer width being exactly 4
    assert!(value == 4);

    value
};
const PTR_TYPE: ValueType = ValueType::I32;

pub const STACK_POINTER_GLOBAL_ID: u32 = 0;
pub const FRAME_ALIGNMENT_BYTES: i32 = 16;
pub const MEMORY_NAME: &str = "memory";
pub const BUILTINS_IMPORT_MODULE_NAME: &str = "env";
pub const STACK_POINTER_NAME: &str = "__stack_pointer";

pub struct Env<'a> {
    pub arena: &'a Bump,
    pub module_id: ModuleId,
    pub exposed_to_host: MutSet<Symbol>,
}

/// Entry point for Roc CLI
pub fn build_module<'a>(
    env: &'a Env<'a>,
    interns: &'a mut Interns,
    preload_bytes: &[u8],
    procedures: MutMap<(Symbol, ProcLayout<'a>), Proc<'a>>,
) -> std::vec::Vec<u8> {
    let (mut wasm_module, called_preload_fns, _) =
        build_module_without_wrapper(env, interns, preload_bytes, procedures);

    wasm_module.remove_dead_preloads(env.arena, called_preload_fns);

    let mut buffer = std::vec::Vec::with_capacity(wasm_module.size());
    wasm_module.serialize(&mut buffer);
    buffer
}

/// Entry point for REPL (repl_wasm) and integration tests (test_gen)
pub fn build_module_without_wrapper<'a>(
    env: &'a Env<'a>,
    interns: &'a mut Interns,
    preload_bytes: &[u8],
    procedures: MutMap<(Symbol, ProcLayout<'a>), Proc<'a>>,
) -> (WasmModule<'a>, Vec<'a, u32>, u32) {
    let mut layout_ids = LayoutIds::default();
    let mut procs = Vec::with_capacity_in(procedures.len(), env.arena);
    let mut proc_lookup = Vec::with_capacity_in(procedures.len() * 2, env.arena);
    let mut linker_symbols = Vec::with_capacity_in(procedures.len() * 2, env.arena);
    let mut exports = Vec::with_capacity_in(4, env.arena);
    let mut maybe_main_fn_index = None;

    // Collect the symbols & names for the procedures,
    // and filter out procs we're going to inline
    let mut fn_index: u32 = 0;
    for ((sym, proc_layout), proc) in procedures.into_iter() {
        if matches!(
            LowLevelWrapperType::from_symbol(sym),
            LowLevelWrapperType::CanBeReplacedBy(_)
        ) {
            continue;
        }
        procs.push(proc);

        let fn_name = layout_ids
            .get_toplevel(sym, &proc_layout)
            .to_symbol_string(sym, interns);

        if env.exposed_to_host.contains(&sym) {
            maybe_main_fn_index = Some(fn_index);
            exports.push(Export {
                name: env.arena.alloc_slice_copy(fn_name.as_bytes()),
                ty: ExportType::Func,
                index: fn_index,
            });
        }

        let linker_sym = SymInfo::for_function(fn_index, fn_name);
        let linker_sym_index = linker_symbols.len() as u32;

        // linker_sym_index is redundant for these procs from user code, but needed for generated helpers!
        proc_lookup.push(ProcLookupData {
            name: sym,
            layout: proc_layout,
            linker_index: linker_sym_index,
            source: ProcSource::Roc,
        });
        linker_symbols.push(linker_sym);

        fn_index += 1;
    }

    // Pre-load the WasmModule with data from the platform & builtins object file
    let initial_module = WasmModule::preload(env.arena, preload_bytes);

    // Adjust Wasm function indices to account for functions from the object file
    let fn_index_offset: u32 =
        initial_module.import.function_count + initial_module.code.preloaded_count;

    let mut backend = WasmBackend::new(
        env,
        interns,
        layout_ids,
        proc_lookup,
        initial_module,
        fn_index_offset,
        CodeGenHelp::new(env.arena, TargetInfo::default_wasm32(), env.module_id),
    );

    if DEBUG_LOG_SETTINGS.user_procs_ir {
        println!("## procs");
        for proc in procs.iter() {
            println!("{}", proc.to_pretty(200));
            // println!("{:?}", proc);
        }
    }

    // Generate procs from user code
    for proc in procs.iter() {
        backend.build_proc(proc);
    }

    // Generate specialized helpers for refcounting & equality
    let helper_procs = backend.get_helpers();

    backend.register_symbol_debug_names();

    if DEBUG_LOG_SETTINGS.helper_procs_ir {
        println!("## helper_procs");
        for proc in helper_procs.iter() {
            println!("{}", proc.to_pretty(200));
            // println!("{:#?}", proc);
        }
    }

    // Generate Wasm for helpers and Zig/Roc wrappers
    let sources = Vec::from_iter_in(
        backend
            .proc_lookup
            .iter()
            .map(|ProcLookupData { source, .. }| *source),
        env.arena,
    );
    let mut helper_iter = helper_procs.iter();
    for source in sources {
        use ProcSource::*;
        match source {
            Roc => { /* already generated */ }
            Helper => {
                if let Some(proc) = helper_iter.next() {
                    backend.build_proc(proc);
                }
            }
            ZigCallConvWrapper => {
                todo!("Generate Wasm wrapper to convert from Zig CC to CCC");
            }
        }
    }

    let (module, called_preload_fns) = backend.finalize();
    let main_function_index = maybe_main_fn_index.unwrap() + fn_index_offset;

    (module, called_preload_fns, main_function_index)
}

pub struct CopyMemoryConfig {
    from_ptr: LocalId,
    from_offset: u32,
    to_ptr: LocalId,
    to_offset: u32,
    size: u32,
    alignment_bytes: u32,
}

pub fn copy_memory(code_builder: &mut CodeBuilder, config: CopyMemoryConfig) {
    if config.from_ptr == config.to_ptr && config.from_offset == config.to_offset {
        return;
    }
    if config.size == 0 {
        return;
    }

    let alignment = Align::from(config.alignment_bytes);
    let mut i = 0;
    while config.size - i >= 8 {
        code_builder.get_local(config.to_ptr);
        code_builder.get_local(config.from_ptr);
        code_builder.i64_load(alignment, i + config.from_offset);
        code_builder.i64_store(alignment, i + config.to_offset);
        i += 8;
    }
    if config.size - i >= 4 {
        code_builder.get_local(config.to_ptr);
        code_builder.get_local(config.from_ptr);
        code_builder.i32_load(alignment, i + config.from_offset);
        code_builder.i32_store(alignment, i + config.to_offset);
        i += 4;
    }
    while config.size - i > 0 {
        code_builder.get_local(config.to_ptr);
        code_builder.get_local(config.from_ptr);
        code_builder.i32_load8_u(alignment, i + config.from_offset);
        code_builder.i32_store8(alignment, i + config.to_offset);
        i += 1;
    }
}

/// Round up to alignment_bytes (which must be a power of 2)
#[macro_export]
macro_rules! round_up_to_alignment {
    ($unaligned: expr, $alignment_bytes: expr) => {
        if $alignment_bytes <= 1 {
            $unaligned
        } else if $alignment_bytes.count_ones() != 1 {
            internal_error!(
                "Cannot align to {} bytes. Not a power of 2.",
                $alignment_bytes
            );
        } else {
            let mut aligned = $unaligned;
            aligned += $alignment_bytes - 1; // if lower bits are non-zero, push it over the next boundary
            aligned &= !$alignment_bytes + 1; // mask with a flag that has upper bits 1, lower bits 0
            aligned
        }
    };
}

pub struct WasmDebugLogSettings {
    proc_start_end: bool,
    user_procs_ir: bool,
    helper_procs_ir: bool,
    let_stmt_ir: bool,
    instructions: bool,
    pub keep_test_binary: bool,
}

pub const DEBUG_LOG_SETTINGS: WasmDebugLogSettings = WasmDebugLogSettings {
    proc_start_end: false && cfg!(debug_assertions),
    user_procs_ir: false && cfg!(debug_assertions),
    helper_procs_ir: false && cfg!(debug_assertions),
    let_stmt_ir: false && cfg!(debug_assertions),
    instructions: false && cfg!(debug_assertions),
    keep_test_binary: false && cfg!(debug_assertions),
};
