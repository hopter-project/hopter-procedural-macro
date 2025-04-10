//! Procedual macro implementations for the [`#[main]`](main)
//! and [`#[handler(IRQ)]`](handler) attribute macro.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    parse_macro_input, AttributeArgs, ItemFn, Meta, NestedMeta, ReturnType, Signature, Type,
};

/// Mark a function as the entry function of the main task.
///
/// The function should satisfy the following signature requirements:
/// - Has one and only one argument of type `cortex_m::Peripherals`.
/// - Returns `()` or `!`.
/// - Is not `async`.
/// - Is not `unsafe`.
/// - Is not variadic.
///
/// Example:
/// ```rust
/// #[main]
/// fn main(cp: cortex_m::Peripherals) {
///    /* initialize system */
///    /* create other tasks */
/// }
/// ```
///
/// The macro works by generating a trampoline function to call the user
/// defined main function. The macro expands to the following for the above
/// example:
///
/// ```rust
/// #[no_mangle]
/// extern "Rust" fn __main_trampoline(arg: AtomicPtr<u8>) {
///     let arg = arg.load(Ordering::SeqCst) as *mut cortex_m::Peripherals;
///     let arg = unsafe { Box::from_raw(arg) };
///     main(*arg)
/// }
/// ```
#[proc_macro_attribute]
pub fn main(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the `item` TokenStream into a Rust function.
    let main_func = parse_macro_input!(item as ItemFn);

    check_main_function_signature(&main_func.sig);

    // Store the function's name.
    let func_name = main_func.sig.ident.to_string();

    // Generate the trampoline function string.
    let trampoline = format!(
        "\
        #[no_mangle]\n\
        extern \"Rust\" fn __main_trampoline(arg: core::sync::atomic::AtomicPtr<u8>) {{\n\
            let arg = arg.load(core::sync::atomic::Ordering::SeqCst) as *mut cortex_m::Peripherals;\n\
            let arg = unsafe {{ alloc::boxed::Box::from_raw(arg) }};\n\
            {}(*arg)\n\
        }}",
        func_name
    );

    // Parse the trampoline string into a token stream.
    let trampoline = syn::parse_str::<TokenStream2>(trampoline.as_str()).unwrap();

    // Output the trampoline followed by the original main function.
    quote! {
        #trampoline
        #main_func
    }
    .into()
}

/// Mark a function as the handler function of an IRQ.
///
/// A handler function should satisfy the following signature requirements:
/// - Has no argument.
/// - Returns `()`.
/// - Is not `async`.
/// - Is not variadic.
///
/// Example:
/// ```rust
/// #[handler(TIM2)]
/// fn tim2_handler() {
///     /* handler logic */
/// }
/// ```
///
/// The macro works by generating an assembly entry sequence and a trampoline
/// function for the IRQ to call the user defined handler function. For example,
/// for `TIM2`, the generated entry sequence and trampoline looks like below:
///
/// ```rust
/// #[naked]
/// #[export_name = "TIM2"]
/// unsafe extern "C" fn __hopter_tim2_entry() {
///     core::arch::asm!(
///         // Preserve the task local storage (TLS) fields and exception return value.
///         "ldr   r0, ={tls_mem_addr}",
///         "ldmia r0!, {{r1-r3}}",
///         "push  {{r1-r3, lr}}",
///         // Set the kernel stacklet boundary and clear out other fields in the TLS.
///         "ldr   r0, ={tls_mem_addr}",
///         "ldr   r1, ={cont_stk_boundary}",
///         "movs  r2, #0",
///         "str   r1, [r0]",
///         "str   r2, [r0, #4]",
///         "str   r2, [r0, #8]",
///         // Run the IRQ handler.
///         "bl    {handler_trampoline}",
///         // Restore the TLS fields and exception return value.
///         "pop   {{r1-r3}}",
///         "ldr   r0, ={tls_mem_addr}",
///         "stmia r0!, {{r1-r3}}",
///         // Exception return.
///         "pop   {{pc}}",
///         tls_mem_addr = const hopter::config::__TLS_MEM_ADDR,
///         cont_stk_boundary = const hopter::config::__CONTIGUOUS_STACK_BOUNDARY,
///         handler_trampoline = sym __hopter_tim2_trampoline,
///         options(noreturn)
///     )
/// }
///
/// unsafe extern "C" fn __hopter_tim2_trampoline() {
///     let prev_is_handler_unwinding
///         = hopter::unwind::unwind::save_and_clear_isr_unwinding();
///     let _ = hopter::unwind::unw_catch::catch_unwind(tim2_handler);
///     hopter::unwind::unwind::set_isr_unwinding(prev_is_handler_unwinding);
/// }
/// ```
#[cfg(feature = "armv6m")]
#[proc_macro_attribute]
pub fn handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the `item` TokenStream into a Rust function.
    let handler_func = parse_macro_input!(item as ItemFn);

    // Parse the `attr` TokenStream into attribute arguments.
    let attr_args = parse_macro_input!(attr as AttributeArgs);

    check_handler_function_signature(&handler_func.sig);

    let irq = parse_attribute_arg_to_irq(&attr_args);
    let lower_caes_irq = irq.to_lowercase();

    // Store the handler function's name.
    let func_name = handler_func.sig.ident.to_string();

    let entry_asm = format!(
        "\
        #[naked]\n\
        #[export_name = \"{}\"]\n\
        unsafe extern \"C\" fn __hopter_{}_entry() {{\n\
            core::arch::asm!(\n\
                \"ldr   r0, ={{tls_mem_addr}}\",\n\
                \"ldmia r0!, {{{{r1-r3}}}}\",\n\
                \"push  {{{{r1-r3, lr}}}}\",\n\
                \"ldr   r0, ={{tls_mem_addr}}\",\n\
                \"ldr   r1, ={{cont_stk_boundary}}\",\n\
                \"movs  r2, #0\",\n\
                \"str   r1, [r0]\",\n\
                \"str   r2, [r0, #4]\",\n\
                \"str   r2, [r0, #8]\",\n\
                // Run the IRQ handler.\n\
                \"bl    {{handler_trampoline}}\",\n\
                \"pop   {{{{r1-r3}}}}\",\n\
                \"ldr   r0, ={{tls_mem_addr}}\",\n\
                \"stmia r0!, {{{{r1-r3}}}}\",\n\
                // Exception return.\n\
                \"pop   {{{{pc}}}}\",\n\
                tls_mem_addr = const hopter::config::__TLS_MEM_ADDR,\n\
                cont_stk_boundary = const hopter::config::__CONTIGUOUS_STACK_BOUNDARY,\n\
                handler_trampoline = sym __hopter_{}_trampoline,\n\
                options(noreturn)\n\
            )\n\
        }}\n\
        ",
        irq, lower_caes_irq, lower_caes_irq,
    );

    let entry_asm = syn::parse_str::<TokenStream2>(entry_asm.as_str()).unwrap();

    let trampoline = format!(
        "\
        unsafe extern \"C\" fn __hopter_{}_trampoline() {{\n\
        let prev_is_handler_unwinding = hopter::unwind::unwind::save_and_clear_isr_unwinding();\n\
        let _ = hopter::unwind::unw_catch::catch_unwind({});\n\
        hopter::unwind::unwind::set_isr_unwinding(prev_is_handler_unwinding);\n\
        }}\n\
        ",
        lower_caes_irq, func_name,
    );

    // Parse the trampoline string into a token stream.
    let trampoline = syn::parse_str::<TokenStream2>(trampoline.as_str()).unwrap();

    // Output the trampoline followed by the original main function.
    quote! {
        #entry_asm
        #trampoline
        #handler_func
    }
    .into()
}

/// Mark a function as the handler function of an IRQ.
///
/// A handler function should satisfy the following signature requirements:
/// - Has no argument.
/// - Returns `()`.
/// - Is not `async`.
/// - Is not variadic.
///
/// Example:
/// ```rust
/// #[handler(TIM2)]
/// fn tim2_handler() {
///     /* handler logic */
/// }
/// ```
///
/// The macro works by generating an assembly entry sequence and a trampoline
/// function for the IRQ to call the user defined handler function. For example,
/// for `TIM2`, the generated entry sequence and trampoline looks like below:
///
/// ```rust
/// #[naked]
/// #[export_name = "TIM2"]
/// unsafe extern "C" fn __hopter_tim2_entry() {
///     core::arch::asm!(
///         // Preserve the task local storage (TLS) fields and exception return value.
///         "ldr   r0, ={tls_mem_addr}",
///         "ldmia r0, {{r1-r3}}",
///         "push  {{r1-r3, lr}}",
///         // Set the kernel stacklet boundary and clear out other fields in the TLS.
///         "ldr   r1, ={cont_stk_boundary}",
///         "mov   r2, #0",
///         "strd  r1, r2, [r0]",
///         "str   r2, [r0, #8]",
///         // Run the IRQ handler.
///         "bl    {handler_trampoline}",
///         // Restore the TLS fields and exception return value.
///         "pop   {{r1-r3}}",
///         "ldr   r0, ={tls_mem_addr}",
///         "stmia r0, {{r1-r3}}",
///         // Exception return.
///         "pop   {{pc}}",
///         tls_mem_addr = const hopter::config::__TLS_MEM_ADDR,
///         cont_stk_boundary = const hopter::config::__CONTIGUOUS_STACK_BOUNDARY,
///         handler_trampoline = sym __hopter_tim2_trampoline,
///         options(noreturn)
///     )
/// }
///
/// unsafe extern "C" fn __hopter_tim2_trampoline() {
///     let prev_is_handler_unwinding
///         = hopter::unwind::unwind::save_and_clear_isr_unwinding();
///     let _ = hopter::unwind::unw_catch::catch_unwind(tim2_handler);
///     hopter::unwind::unwind::set_isr_unwinding(prev_is_handler_unwinding);
/// }
/// ```
#[cfg(not(feature = "armv6m"))]
#[proc_macro_attribute]
pub fn handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the `item` TokenStream into a Rust function.
    let handler_func = parse_macro_input!(item as ItemFn);

    // Parse the `attr` TokenStream into attribute arguments.
    let attr_args = parse_macro_input!(attr as AttributeArgs);

    check_handler_function_signature(&handler_func.sig);

    let irq = parse_attribute_arg_to_irq(&attr_args);
    let lower_caes_irq = irq.to_lowercase();

    // Store the handler function's name.
    let func_name = handler_func.sig.ident.to_string();

    let entry_asm = format!(
        "\
        #[naked]\n\
        #[export_name = \"{}\"]\n\
        unsafe extern \"C\" fn __hopter_{}_entry() {{\n\
            core::arch::asm!(\n\
                \"ldr   r0, ={{tls_mem_addr}}\",\n\
                \"ldmia r0, {{{{r1-r3}}}}\",\n\
                \"push  {{{{r1-r3, lr}}}}\",\n\
                \"ldr   r1, ={{cont_stk_boundary}}\",\n\
                \"mov   r2, #0\",\n\
                \"strd  r1, r2, [r0]\",\n\
                \"str   r2, [r0, #8]\",\n\
                // Run the IRQ handler.\n\
                \"bl    {{handler_trampoline}}\",\n\
                \"pop   {{{{r1-r3}}}}\",\n\
                \"ldr   r0, ={{tls_mem_addr}}\",\n\
                \"stmia r0, {{{{r1-r3}}}}\",\n\
                // Exception return.\n\
                \"pop   {{{{pc}}}}\",\n\
                tls_mem_addr = const hopter::config::__TLS_MEM_ADDR,\n\
                cont_stk_boundary = const hopter::config::__CONTIGUOUS_STACK_BOUNDARY,\n\
                handler_trampoline = sym __hopter_{}_trampoline,\n\
                options(noreturn)\n\
            )\n\
        }}\n\
        ",
        irq, lower_caes_irq, lower_caes_irq,
    );

    let entry_asm = syn::parse_str::<TokenStream2>(entry_asm.as_str()).unwrap();

    let trampoline = format!(
        "\
        unsafe extern \"C\" fn __hopter_{}_trampoline() {{\n\
        let prev_is_handler_unwinding = hopter::unwind::unwind::save_and_clear_isr_unwinding();\n\
        let _ = hopter::unwind::unw_catch::catch_unwind({});\n\
        hopter::unwind::unwind::set_isr_unwinding(prev_is_handler_unwinding);\n\
        }}\n\
        ",
        lower_caes_irq, func_name,
    );

    // Parse the trampoline string into a token stream.
    let trampoline = syn::parse_str::<TokenStream2>(trampoline.as_str()).unwrap();

    // Output the trampoline followed by the original main function.
    quote! {
        #entry_asm
        #trampoline
        #handler_func
    }
    .into()
}

macro_rules! hander_macro_arg_error {
    () => {
        "Handler's argument must be one of the supported IRQs. Forgot to set the MCU model feature?"
    };
}

macro_rules! hander_macro_retval_error {
    () => {
        "Handler's return type must be ()."
    };
}

/// The main function should satisfy the following signature requirements:
/// - Has one and only one argument of type `cortex_m::Peripherals`.
/// - Returns `()` or `!`.
/// - Is not `async`.
/// - Is not `unsafe`.
/// - Is not variadic.
fn check_main_function_signature(sig: &Signature) {
    if sig.inputs.iter().count() != 1 {
        panic!("Main function must receive one argument of type `cortex_m::Peripherals`.");
    }

    match &sig.output {
        // No return type specification.
        ReturnType::Default => {}
        // Specified return type as `-> ()`.
        ReturnType::Type(_, b) => match &**b {
            Type::Tuple(t) => {
                if t.elems.len() != 0 {
                    panic!(hander_macro_retval_error!());
                }
            }
            Type::Never(_) => {}
            _ => panic!(hander_macro_retval_error!()),
        },
    }

    if sig.asyncness.is_some() {
        panic!("Main function cannot be `async`.");
    }

    if sig.unsafety.is_some() {
        panic!("Main function must be safe.");
    }

    if sig.variadic.is_some() {
        panic!("Handler function cannot be variadic.");
    }
}

/// A handler function should satisfy the following signature requirements:
/// - Has no argument.
/// - Returns `()`.
/// - Is not `async`.
/// - Is not variadic.
fn check_handler_function_signature(sig: &Signature) {
    if sig.inputs.iter().count() != 0 {
        panic!("Handler function should not have any parameter.");
    }

    match &sig.output {
        // No return type specification.
        ReturnType::Default => {}
        // Specified return type as `-> ()`.
        ReturnType::Type(_, b) => match &**b {
            Type::Tuple(t) => {
                if t.elems.len() != 0 {
                    panic!(hander_macro_retval_error!());
                }
            }
            _ => panic!(hander_macro_retval_error!()),
        },
    }

    if sig.abi.is_some() {
        panic!("Handler function must have Rust ABI.");
    }

    if sig.asyncness.is_some() {
        panic!("Handler function cannot be `async`.");
    }

    if sig.variadic.is_some() {
        panic!("Handler function cannot be variadic.");
    }
}

/// The handler attribute should contain one and only one argument, which is
/// a supported IRQ name.
fn parse_attribute_arg_to_irq(attr_args: &[NestedMeta]) -> String {
    // Check that there is only one attribute argument.
    if attr_args.len() != 1 {
        panic!(hander_macro_arg_error!());
    }

    // Convert the argument into a string.
    let arg = match attr_args.first().unwrap() {
        NestedMeta::Meta(Meta::Path(ss)) => quote! { #ss }.to_string(),
        _ => panic!(hander_macro_arg_error!()),
    };

    // Verify that the string names one of the supported IRQs.
    if !SUPPORTED_IRQS.iter().any(|irq| irq == &arg) {
        panic!(hander_macro_arg_error!());
    }

    arg
}

#[cfg(any(
    feature = "stm32f401",
    feature = "stm32f405",
    feature = "stm32f407",
    feature = "stm32f410",
    feature = "stm32f411",
    feature = "stm32f412",
    feature = "stm32f413",
    feature = "stm32f427",
    feature = "stm32f429",
    feature = "stm32f446",
    feature = "stm32f469",
))]
mod stm32f4;

#[cfg(any(
    feature = "stm32f401",
    feature = "stm32f405",
    feature = "stm32f407",
    feature = "stm32f410",
    feature = "stm32f411",
    feature = "stm32f412",
    feature = "stm32f413",
    feature = "stm32f427",
    feature = "stm32f429",
    feature = "stm32f446",
    feature = "stm32f469",
))]
use stm32f4::SUPPORTED_IRQS;

#[cfg(any(
    feature = "stm32f030",
    feature = "stm32f030x4",
    feature = "stm32f030x6",
    feature = "stm32f030x8",
    feature = "stm32f030xc",
    feature = "stm32f031",
    feature = "stm32f038",
    feature = "stm32f042",
    feature = "stm32f048",
    feature = "stm32f051",
    feature = "stm32f058",
    feature = "stm32f070",
    feature = "stm32f070x6",
    feature = "stm32f070xb",
    feature = "stm32f071",
    feature = "stm32f072",
    feature = "stm32f078",
    feature = "stm32f091",
    feature = "stm32f098",
))]
mod stm32f0;

#[cfg(any(
    feature = "stm32f030",
    feature = "stm32f030x4",
    feature = "stm32f030x6",
    feature = "stm32f030x8",
    feature = "stm32f030xc",
    feature = "stm32f031",
    feature = "stm32f038",
    feature = "stm32f042",
    feature = "stm32f048",
    feature = "stm32f051",
    feature = "stm32f058",
    feature = "stm32f070",
    feature = "stm32f070x6",
    feature = "stm32f070xb",
    feature = "stm32f071",
    feature = "stm32f072",
    feature = "stm32f078",
    feature = "stm32f091",
    feature = "stm32f098",
))]
use stm32f0::SUPPORTED_IRQS;

#[cfg(not(any(
    feature = "stm32f401",
    feature = "stm32f405",
    feature = "stm32f407",
    feature = "stm32f410",
    feature = "stm32f411",
    feature = "stm32f412",
    feature = "stm32f413",
    feature = "stm32f427",
    feature = "stm32f429",
    feature = "stm32f446",
    feature = "stm32f469",
    feature = "stm32f030",
    feature = "stm32f030x4",
    feature = "stm32f030x6",
    feature = "stm32f030x8",
    feature = "stm32f030xc",
    feature = "stm32f031",
    feature = "stm32f038",
    feature = "stm32f042",
    feature = "stm32f048",
    feature = "stm32f051",
    feature = "stm32f058",
    feature = "stm32f070",
    feature = "stm32f070x6",
    feature = "stm32f070xb",
    feature = "stm32f071",
    feature = "stm32f072",
    feature = "stm32f078",
    feature = "stm32f091",
    feature = "stm32f098",
)))]
const SUPPORTED_IRQS: [&str; 0] = [];
