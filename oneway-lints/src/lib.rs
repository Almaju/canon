#![feature(rustc_private)]
#![allow(unused_extern_crates)]

extern crate rustc_ast;
extern crate rustc_lint;
extern crate rustc_session;
extern crate rustc_span;

dylint_linting::dylint_library!();

mod control_flow;
mod functions;
mod safety;
mod sorting;
mod style;

#[doc(hidden)]
#[no_mangle]
pub fn register_lints(_sess: &rustc_session::Session, lint_store: &mut rustc_lint::LintStore) {
    // Sorting
    lint_store.register_lints(&[
        sorting::UNSORTED_STRUCT_FIELDS,
        sorting::UNSORTED_ENUM_VARIANTS,
        sorting::UNSORTED_MATCH_ARMS,
        sorting::UNSORTED_IMPORTS,
        sorting::UNSORTED_IMPL_METHODS,
        sorting::UNSORTED_DERIVES,
    ]);
    lint_store.register_early_pass(|| Box::new(sorting::UnsortedStructFields));
    lint_store.register_early_pass(|| Box::new(sorting::UnsortedEnumVariants));
    lint_store.register_early_pass(|| Box::new(sorting::UnsortedMatchArms));
    lint_store.register_early_pass(|| Box::new(sorting::UnsortedImports));
    lint_store.register_early_pass(|| Box::new(sorting::UnsortedImplMethods));
    lint_store.register_early_pass(|| Box::new(sorting::UnsortedDerives));

    // Functions
    lint_store.register_lints(&[
        functions::TOO_MANY_PARAMS,
        functions::NO_NESTED_FUNCTIONS,
        functions::ONE_CONSTRUCTOR_NAME,
    ]);
    lint_store.register_early_pass(|| Box::new(functions::TooManyParams));
    lint_store.register_early_pass(|| Box::new(functions::NoNestedFunctions));
    lint_store.register_early_pass(|| Box::new(functions::OneConstructorName));

    // Control flow
    lint_store.register_lints(&[
        control_flow::NO_LOOP,
        control_flow::NO_IF_ELSE,
        control_flow::NO_EXPLICIT_RETURN,
    ]);
    lint_store.register_early_pass(|| Box::new(control_flow::NoLoop));
    lint_store.register_early_pass(|| Box::new(control_flow::NoIfElse));
    lint_store.register_early_pass(|| Box::new(control_flow::NoExplicitReturn));

    // Safety
    lint_store.register_lints(&[safety::NO_UNWRAP, safety::NO_PANIC]);
    lint_store.register_early_pass(|| Box::new(safety::NoUnwrap));
    lint_store.register_early_pass(|| Box::new(safety::NoPanic));

    // Style
    lint_store.register_lints(&[style::NO_GLOB_IMPORTS, style::NO_COMMENTS]);
    lint_store.register_early_pass(|| Box::new(style::NoGlobImports));
    lint_store.register_early_pass(|| Box::new(style::NoComments));
}
