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

/// List of supported IRQ names.

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

#[cfg(feature = "stm32f401")]
const SUPPORTED_IRQS: [&str; 55] = [
    "PVD",
    "TAMP_STAMP",
    "RTC_WKUP",
    "FLASH",
    "RCC",
    "EXTI0",
    "EXTI1",
    "EXTI2",
    "EXTI3",
    "EXTI4",
    "DMA1_STREAM0",
    "DMA1_STREAM1",
    "DMA1_STREAM2",
    "DMA1_STREAM3",
    "DMA1_STREAM4",
    "DMA1_STREAM5",
    "DMA1_STREAM6",
    "ADC",
    "EXTI9_5",
    "TIM1_BRK_TIM9",
    "TIM1_UP_TIM10",
    "TIM1_TRG_COM_TIM11",
    "TIM1_CC",
    "TIM2",
    "TIM3",
    "TIM4",
    "I2C1_EV",
    "I2C1_ER",
    "I2C2_EV",
    "I2C2_ER",
    "SPI1",
    "SPI2",
    "USART1",
    "USART2",
    "EXTI15_10",
    "RTC_ALARM",
    "OTG_FS_WKUP",
    "DMA1_STREAM7",
    "SDIO",
    "TIM5",
    "SPI3",
    "DMA2_STREAM0",
    "DMA2_STREAM1",
    "DMA2_STREAM2",
    "DMA2_STREAM3",
    "DMA2_STREAM4",
    "OTG_FS",
    "DMA2_STREAM5",
    "DMA2_STREAM6",
    "DMA2_STREAM7",
    "USART6",
    "I2C3_EV",
    "I2C3_ER",
    "FPU",
    "SPI4",
];

#[cfg(feature = "stm32f405")]
const SUPPORTED_IRQS: [&str; 83] = [
    "WWDG",
    "PVD",
    "TAMP_STAMP",
    "RTC_WKUP",
    "RCC",
    "EXTI0",
    "EXTI1",
    "EXTI2",
    "EXTI3",
    "EXTI4",
    "DMA1_STREAM0",
    "DMA1_STREAM1",
    "DMA1_STREAM2",
    "DMA1_STREAM3",
    "DMA1_STREAM4",
    "DMA1_STREAM5",
    "DMA1_STREAM6",
    "ADC",
    "CAN1_TX",
    "CAN1_RX0",
    "CAN1_RX1",
    "CAN1_SCE",
    "EXTI9_5",
    "TIM1_BRK_TIM9",
    "TIM1_UP_TIM10",
    "TIM1_TRG_COM_TIM11",
    "TIM1_CC",
    "TIM2",
    "TIM3",
    "TIM4",
    "I2C1_EV",
    "I2C1_ER",
    "I2C2_EV",
    "I2C2_ER",
    "SPI1",
    "SPI2",
    "USART1",
    "USART2",
    "USART3",
    "EXTI15_10",
    "RTC_ALARM",
    "OTG_FS_WKUP",
    "TIM8_BRK_TIM12",
    "TIM8_UP_TIM13",
    "TIM8_TRG_COM_TIM14",
    "TIM8_CC",
    "DMA1_STREAM7",
    "FSMC",
    "SDIO",
    "TIM5",
    "SPI3",
    "UART4",
    "UART5",
    "TIM6_DAC",
    "TIM7",
    "DMA2_STREAM0",
    "DMA2_STREAM1",
    "DMA2_STREAM2",
    "DMA2_STREAM3",
    "DMA2_STREAM4",
    "ETH",
    "ETH_WKUP",
    "CAN2_TX",
    "CAN2_RX0",
    "CAN2_RX1",
    "CAN2_SCE",
    "OTG_FS",
    "DMA2_STREAM5",
    "DMA2_STREAM6",
    "DMA2_STREAM7",
    "USART6",
    "I2C3_EV",
    "I2C3_ER",
    "OTG_HS_EP1_OUT",
    "OTG_HS_EP1_IN",
    "OTG_HS_WKUP",
    "OTG_HS",
    "DCMI",
    "CRYP",
    "HASH_RNG",
    "FPU",
    "LTDC",
    "LTDC_ER",
];

#[cfg(feature = "stm32f407")]
const SUPPORTED_IRQS: [&str; 83] = [
    "WWDG",
    "PVD",
    "TAMP_STAMP",
    "RTC_WKUP",
    "RCC",
    "EXTI0",
    "EXTI1",
    "EXTI2",
    "EXTI3",
    "EXTI4",
    "DMA1_STREAM0",
    "DMA1_STREAM1",
    "DMA1_STREAM2",
    "DMA1_STREAM3",
    "DMA1_STREAM4",
    "DMA1_STREAM5",
    "DMA1_STREAM6",
    "ADC",
    "CAN1_TX",
    "CAN1_RX0",
    "CAN1_RX1",
    "CAN1_SCE",
    "EXTI9_5",
    "TIM1_BRK_TIM9",
    "TIM1_UP_TIM10",
    "TIM1_TRG_COM_TIM11",
    "TIM1_CC",
    "TIM2",
    "TIM3",
    "TIM4",
    "I2C1_EV",
    "I2C1_ER",
    "I2C2_EV",
    "I2C2_ER",
    "SPI1",
    "SPI2",
    "USART1",
    "USART2",
    "USART3",
    "EXTI15_10",
    "RTC_ALARM",
    "OTG_FS_WKUP",
    "TIM8_BRK_TIM12",
    "TIM8_UP_TIM13",
    "TIM8_TRG_COM_TIM14",
    "TIM8_CC",
    "DMA1_STREAM7",
    "FSMC",
    "SDIO",
    "TIM5",
    "SPI3",
    "UART4",
    "UART5",
    "TIM6_DAC",
    "TIM7",
    "DMA2_STREAM0",
    "DMA2_STREAM1",
    "DMA2_STREAM2",
    "DMA2_STREAM3",
    "DMA2_STREAM4",
    "ETH",
    "ETH_WKUP",
    "CAN2_TX",
    "CAN2_RX0",
    "CAN2_RX1",
    "CAN2_SCE",
    "OTG_FS",
    "DMA2_STREAM5",
    "DMA2_STREAM6",
    "DMA2_STREAM7",
    "USART6",
    "I2C3_EV",
    "I2C3_ER",
    "OTG_HS_EP1_OUT",
    "OTG_HS_EP1_IN",
    "OTG_HS_WKUP",
    "OTG_HS",
    "DCMI",
    "CRYP",
    "HASH_RNG",
    "FPU",
    "LCD_TFT",
    "LCD_TFT_1",
];

#[cfg(feature = "stm32f410")]
const SUPPORTED_IRQS: [&str; 54] = [
    "WWDG",
    "PVD",
    "TAMP_STAMP",
    "RTC_WKUP",
    "FLASH",
    "RCC",
    "EXTI0",
    "EXTI1",
    "EXTI2",
    "EXTI3",
    "EXTI4",
    "DMA1_STREAM0",
    "DMA1_STREAM1",
    "DMA1_STREAM2",
    "DMA1_STREAM3",
    "DMA1_STREAM4",
    "DMA1_STREAM5",
    "DMA1_STREAM6",
    "ADC",
    "EXTI9_5",
    "TIM1_BRK_TIM9",
    "PWM1_UP",
    "TIM1_TRG_COM_TIM11",
    "TIM1_CC",
    "I2C1_EV",
    "I2C1_ER",
    "I2C2_EV",
    "I2C2_ER",
    "SPI1",
    "SPI2",
    "USART1",
    "USART2",
    "EXTI15_10",
    "RTC_ALARM",
    "DMA1_STREAM7",
    "TIM5",
    "TIM6_DAC1",
    "DMA2_STREAM0",
    "DMA2_STREAM1",
    "DMA2_STREAM2",
    "DMA2_STREAM3",
    "DMA2_STREAM4",
    "EXTI19",
    "DMA2_STREAM5",
    "DMA2_STREAM6",
    "DMA2_STREAM7",
    "USART6",
    "EXTI20",
    "RNG",
    "FPU",
    "SPI5",
    "I2C4_EV",
    "I2C4_ER",
    "LPTIM1",
];

#[cfg(feature = "stm32f411")]
const SUPPORTED_IRQS: [&str; 57] = [
    "WWDG",
    "PVD",
    "TAMP_STAMP",
    "RTC_WKUP",
    "FLASH",
    "RCC",
    "EXTI0",
    "EXTI1",
    "EXTI2",
    "EXTI3",
    "EXTI4",
    "DMA1_STREAM0",
    "DMA1_STREAM1",
    "DMA1_STREAM2",
    "DMA1_STREAM3",
    "DMA1_STREAM4",
    "DMA1_STREAM5",
    "DMA1_STREAM6",
    "ADC",
    "EXTI9_5",
    "TIM1_BRK_TIM9",
    "TIM1_UP_TIM10",
    "TIM1_TRG_COM_TIM11",
    "TIM1_CC",
    "TIM2",
    "TIM3",
    "TIM4",
    "I2C1_EV",
    "I2C1_ER",
    "I2C2_EV",
    "I2C2_ER",
    "SPI1",
    "SPI2",
    "USART1",
    "USART2",
    "EXTI15_10",
    "RTC_ALARM",
    "OTG_FS_WKUP",
    "DMA1_STREAM7",
    "SDIO",
    "TIM5",
    "SPI3",
    "DMA2_STREAM0",
    "DMA2_STREAM1",
    "DMA2_STREAM2",
    "DMA2_STREAM3",
    "DMA2_STREAM4",
    "OTG_FS",
    "DMA2_STREAM5",
    "DMA2_STREAM6",
    "DMA2_STREAM7",
    "USART6",
    "I2C3_EV",
    "I2C3_ER",
    "FPU",
    "SPI4",
    "SPI5",
];

#[cfg(feature = "stm32f412")]
const SUPPORTED_IRQS: [&str; 79] = [
    "WWDG",
    "PVD",
    "TAMP_STAMP",
    "RTC_WKUP",
    "FLASH",
    "RCC",
    "EXTI0",
    "EXTI1",
    "EXTI2",
    "EXTI3",
    "EXTI4",
    "DMA1_STREAM0",
    "DMA1_STREAM1",
    "DMA1_STREAM2",
    "DMA1_STREAM3",
    "DMA1_STREAM4",
    "DMA1_STREAM5",
    "DMA1_STREAM6",
    "ADC",
    "CAN1_TX",
    "CAN1_RX0",
    "CAN1_RX1",
    "CAN1_SCE",
    "EXTI9_5",
    "TIM1_BRK_TIM9",
    "TIM1_UP_TIM10",
    "TIM1_TRG_COM_TIM11",
    "TIM1_CC",
    "TIM2",
    "TIM3",
    "TIM4",
    "I2C1_EV",
    "I2C1_ER",
    "I2C2_EV",
    "I2C2_ER",
    "SPI1",
    "SPI2",
    "USART1",
    "USART2",
    "USART3",
    "EXTI15_10",
    "RTC_ALARM",
    "OTG_FS_WKUP",
    "TIM12",
    "TIM13",
    "TIM14",
    "TIM8_CC",
    "DMA1_STREAM7",
    "FSMC",
    "SDIO",
    "TIM5",
    "SPI3",
    "TIM6_DACUNDER",
    "TIM7",
    "DMA2_STREAM0",
    "DMA2_STREAM1",
    "DMA2_STREAM2",
    "DMA2_STREAM3",
    "DMA2_STREAM4",
    "DFSDM1_FLT0",
    "DFSDM1_FLT1",
    "CAN2_TX",
    "CAN2_RX0",
    "CAN2_RX1",
    "CAN2_SCE",
    "OTG_FS",
    "DMA2_STREAM5",
    "DMA2_STREAM6",
    "DMA2_STREAM7",
    "USART6",
    "I2C3_EV",
    "I2C3_ER",
    "HASH_RNG",
    "FPU",
    "SPI4",
    "SPI5",
    "QUAD_SPI",
    "I2CFMP1_EVENT",
    "I2CFMP1_ERROR",
];

#[cfg(feature = "stm32f413")]
const SUPPORTED_IRQS: [&str; 94] = [
    "PVD",
    "TAMP_STAMP",
    "RTC_WKUP",
    "FLASH",
    "RCC",
    "EXTI0",
    "EXTI1",
    "EXTI2",
    "EXTI3",
    "EXTI4",
    "DMA1_STREAM0",
    "DMA1_STREAM1",
    "DMA1_STREAM2",
    "DMA1_STREAM3",
    "DMA1_STREAM4",
    "DMA1_STREAM5",
    "DMA1_STREAM6",
    "ADC",
    "CAN1_TX",
    "CAN1_RX0",
    "CAN1_RX1",
    "CAN1_SCE",
    "EXTI9_5",
    "TIM1_BRK_TIM9",
    "TIM1_UP_TIM10",
    "TIM1_TRG_COM_TIM11",
    "TIM1_CC",
    "TIM2",
    "TIM3",
    "TIM4",
    "I2C1_EVT",
    "I2C1_ERR",
    "I2C2_EVT",
    "I2C2_ERR",
    "SPI1",
    "SPI2",
    "USART1",
    "USART2",
    "USART3",
    "EXTI15_10",
    "EXTI17_RTC_ALARM",
    "TIM8_BRK_TIM12",
    "TIM8_UP_TIM13",
    "TIM8_TRG_COM_TIM14",
    "TIM8_CC",
    "DMA1_STREAM7",
    "FSMC",
    "SDIO",
    "TIM5",
    "SPI3",
    "USART4",
    "UART5",
    "TIM6_GLB_IT_DAC1_DAC2",
    "TIM7",
    "DMA2_STREAM0",
    "DMA2_STREAM1",
    "DMA2_STREAM2",
    "DMA2_STREAM3",
    "DMA2_STREAM4",
    "DFSDM1_FLT0",
    "DFSDM1_FLT1",
    "CAN2_TX",
    "CAN2_RX0",
    "CAN2_RX1",
    "CAN2_SCE",
    "OTG_FS",
    "DMA2_STREAM5",
    "DMA2_STREAM6",
    "DMA2_STREAM7",
    "USART6",
    "I2C3_EV",
    "I2C3_ER",
    "CAN3_TX",
    "CAN3_RX0",
    "CAN3_RX1",
    "CAN3_SCE",
    "CRYPTO",
    "RNG",
    "FPU",
    "USART7",
    "USART8",
    "SPI4",
    "SPI5",
    "SAI1",
    "UART9",
    "UART10",
    "QUADSPI",
    "I2CFMP1EVENT",
    "I2CFMP1ERROR",
    "LPTIM1_OR_IT_EIT_23",
    "DFSDM2_FILTER1",
    "DFSDM2_FILTER2",
    "DFSDM2_FILTER3",
    "DFSDM2_FILTER4",
];

#[cfg(feature = "stm32f427")]
const SUPPORTED_IRQS: [&str; 89] = [
    "WWDG",
    "PVD",
    "TAMP_STAMP",
    "RTC_WKUP",
    "FLASH",
    "RCC",
    "EXTI0",
    "EXTI1",
    "EXTI2",
    "EXTI3",
    "EXTI4",
    "DMA1_STREAM0",
    "DMA1_STREAM1",
    "DMA1_STREAM2",
    "DMA1_STREAM3",
    "DMA1_STREAM4",
    "DMA1_STREAM5",
    "DMA1_STREAM6",
    "ADC",
    "CAN1_TX",
    "CAN1_RX0",
    "CAN1_RX1",
    "CAN1_SCE",
    "EXTI9_5",
    "TIM1_BRK_TIM9",
    "TIM1_UP_TIM10",
    "TIM1_TRG_COM_TIM11",
    "TIM1_CC",
    "TIM2",
    "TIM3",
    "TIM4",
    "I2C1_EV",
    "I2C1_ER",
    "I2C2_EV",
    "I2C2_ER",
    "SPI1",
    "SPI2",
    "USART1",
    "USART2",
    "USART3",
    "EXTI15_10",
    "RTC_ALARM",
    "OTG_FS_WKUP",
    "TIM8_BRK_TIM12",
    "TIM8_UP_TIM13",
    "TIM8_TRG_COM_TIM14",
    "TIM8_CC",
    "DMA1_STREAM7",
    "FMC",
    "SDIO",
    "TIM5",
    "SPI3",
    "UART4",
    "UART5",
    "TIM6_DAC",
    "TIM7",
    "DMA2_STREAM0",
    "DMA2_STREAM1",
    "DMA2_STREAM2",
    "DMA2_STREAM3",
    "DMA2_STREAM4",
    "ETH",
    "ETH_WKUP",
    "CAN2_TX",
    "CAN2_RX0",
    "CAN2_RX1",
    "CAN2_SCE",
    "OTG_FS",
    "DMA2_STREAM5",
    "DMA2_STREAM6",
    "DMA2_STREAM7",
    "USART6",
    "I2C3_EV",
    "I2C3_ER",
    "OTG_HS_EP1_OUT",
    "OTG_HS_EP1_IN",
    "OTG_HS_WKUP",
    "OTG_HS",
    "DCMI",
    "CRYP",
    "HASH_RNG",
    "FPU",
    "UART7",
    "UART8",
    "SPI4",
    "SPI5",
    "SPI6",
    "LCD_TFT",
    "LCD_TFT_1",
];

#[cfg(feature = "stm32f429")]
const SUPPORTED_IRQS: [&str; 91] = [
    "WWDG",
    "PVD",
    "TAMP_STAMP",
    "RTC_WKUP",
    "FLASH",
    "RCC",
    "EXTI0",
    "EXTI1",
    "EXTI2",
    "EXTI3",
    "EXTI4",
    "DMA1_STREAM0",
    "DMA1_STREAM1",
    "DMA1_STREAM2",
    "DMA1_STREAM3",
    "DMA1_STREAM4",
    "DMA1_STREAM5",
    "DMA1_STREAM6",
    "ADC",
    "CAN1_TX",
    "CAN1_RX0",
    "CAN1_RX1",
    "CAN1_SCE",
    "EXTI9_5",
    "TIM1_BRK_TIM9",
    "TIM1_UP_TIM10",
    "TIM1_TRG_COM_TIM11",
    "TIM1_CC",
    "TIM2",
    "TIM3",
    "TIM4",
    "I2C1_EV",
    "I2C1_ER",
    "I2C2_EV",
    "I2C2_ER",
    "SPI1",
    "SPI2",
    "USART1",
    "USART2",
    "USART3",
    "EXTI15_10",
    "RTC_ALARM",
    "OTG_FS_WKUP",
    "TIM8_BRK_TIM12",
    "TIM8_UP_TIM13",
    "TIM8_TRG_COM_TIM14",
    "TIM8_CC",
    "DMA1_STREAM7",
    "FMC",
    "SDIO",
    "TIM5",
    "SPI3",
    "UART4",
    "UART5",
    "TIM6_DAC",
    "TIM7",
    "DMA2_STREAM0",
    "DMA2_STREAM1",
    "DMA2_STREAM2",
    "DMA2_STREAM3",
    "DMA2_STREAM4",
    "ETH",
    "ETH_WKUP",
    "CAN2_TX",
    "CAN2_RX0",
    "CAN2_RX1",
    "CAN2_SCE",
    "OTG_FS",
    "DMA2_STREAM5",
    "DMA2_STREAM6",
    "DMA2_STREAM7",
    "USART6",
    "I2C3_EV",
    "I2C3_ER",
    "OTG_HS_EP1_OUT",
    "OTG_HS_EP1_IN",
    "OTG_HS_WKUP",
    "OTG_HS",
    "DCMI",
    "CRYP",
    "HASH_RNG",
    "FPU",
    "UART7",
    "UART8",
    "SPI4",
    "SPI5",
    "SPI6",
    "SAI1",
    "LCD_TFT",
    "LCD_TFT_1",
    "DMA2D",
];

#[cfg(feature = "stm32f446")]
const SUPPORTED_IRQS: [&str; 80] = [
    "WWDG",
    "TAMP_STAMP",
    "RTC_WKUP",
    "FLASH",
    "RCC",
    "EXTI0",
    "EXTI1",
    "EXTI2",
    "EXTI3",
    "EXTI4",
    "DMA1_STREAM0",
    "DMA1_STREAM1",
    "DMA1_STREAM2",
    "DMA1_STREAM3",
    "DMA1_STREAM4",
    "DMA1_STREAM5",
    "DMA1_STREAM6",
    "ADC",
    "CAN1_TX",
    "CAN1_RX0",
    "CAN1_RX1",
    "CAN1_SCE",
    "EXTI9_5",
    "TIM1_BRK_TIM9",
    "TIM1_UP_TIM10",
    "TIM1_TRG_COM_TIM11",
    "TIM1_CC",
    "TIM2",
    "TIM3",
    "TIM4",
    "I2C1_EV",
    "I2C1_ER",
    "I2C2_EV",
    "I2C2_ER",
    "SPI1",
    "SPI2",
    "USART1",
    "USART2",
    "USART3",
    "EXTI15_10",
    "RTC_ALARM",
    "OTG_FS_WKUP",
    "TIM8_BRK_TIM12",
    "TIM8_UP_TIM13",
    "TIM8_TRG_COM_TIM14",
    "TIM8_CC",
    "DMA1_STREAM7",
    "FMC",
    "SDIO",
    "TIM5",
    "SPI3",
    "UART4",
    "UART5",
    "TIM6_DAC",
    "TIM7",
    "DMA2_STREAM0",
    "DMA2_STREAM1",
    "DMA2_STREAM2",
    "DMA2_STREAM3",
    "DMA2_STREAM4",
    "ETH",
    "ETH_WKUP",
    "CAN2_TX",
    "CAN2_RX0",
    "CAN2_RX1",
    "CAN2_SCE",
    "OTG_FS",
    "DMA2_STREAM5",
    "DMA2_STREAM6",
    "DMA2_STREAM7",
    "USART6",
    "I2C3_EV",
    "I2C3_ER",
    "DCMI",
    "FPU",
    "UART7",
    "UART8",
    "SPI4",
    "LCD_TFT",
    "LCD_TFT_1",
];

#[cfg(feature = "stm32f469")]
const SUPPORTED_IRQS: [&str; 93] = [
    "WWDG",
    "PVD",
    "TAMP_STAMP",
    "RTC_WKUP",
    "FLASH",
    "RCC",
    "EXTI0",
    "EXTI1",
    "EXTI2",
    "EXTI3",
    "EXTI4",
    "DMA1_STREAM0",
    "DMA1_STREAM1",
    "DMA1_STREAM2",
    "DMA1_STREAM3",
    "DMA1_STREAM4",
    "DMA1_STREAM5",
    "DMA1_STREAM6",
    "ADC",
    "CAN1_TX",
    "CAN1_RX0",
    "CAN1_RX1",
    "CAN1_SCE",
    "EXTI9_5",
    "TIM1_BRK_TIM9",
    "TIM1_UP_TIM10",
    "TIM1_TRG_COM_TIM11",
    "TIM1_CC",
    "TIM2",
    "TIM3",
    "TIM4",
    "I2C1_EV",
    "I2C1_ER",
    "I2C2_EV",
    "I2C2_ER",
    "SPI1",
    "SPI2",
    "USART1",
    "USART2",
    "USART3",
    "EXTI15_10",
    "RTC_ALARM",
    "OTG_FS_WKUP",
    "TIM8_BRK_TIM12",
    "TIM8_UP_TIM13",
    "TIM8_TRG_COM_TIM14",
    "TIM8_CC",
    "DMA1_STREAM7",
    "FMC",
    "SDIO",
    "TIM5",
    "SPI3",
    "UART4",
    "UART5",
    "TIM6_DAC",
    "TIM7",
    "DMA2_STREAM0",
    "DMA2_STREAM1",
    "DMA2_STREAM2",
    "DMA2_STREAM3",
    "DMA2_STREAM4",
    "ETH",
    "ETH_WKUP",
    "CAN2_TX",
    "CAN2_RX0",
    "CAN2_RX1",
    "CAN2_SCE",
    "OTG_FS",
    "DMA2_STREAM5",
    "DMA2_STREAM6",
    "DMA2_STREAM7",
    "USART6",
    "I2C3_EV",
    "I2C3_ER",
    "OTG_HS_EP1_OUT",
    "OTG_HS_EP1_IN",
    "OTG_HS_WKUP",
    "OTG_HS",
    "DCMI",
    "CRYP",
    "HASH_RNG",
    "FPU",
    "UART7",
    "UART8",
    "SPI4",
    "SPI5",
    "SPI6",
    "SAI1",
    "LCD_TFT",
    "LCD_TFT_1",
    "DMA2D",
    "QUADSPI",
    "DSIHOST",
];

#[cfg(any(feature = "stm32f030", feature = "stm32f070"))]
const SUPPORTED_IRQS: [&str; 28] = [
    "WWDG",
    "PVD",
    "RTC",
    "FLASH",
    "RCC",
    "EXTI0_1",
    "EXTI2_3",
    "EXTI4_15",
    "DMA1_CH1",
    "DMA1_CH2_3",
    "DMA1_CH4_5",
    "ADC",
    "TIM1_BRK_UP_TRG_COM",
    "TIM1_CC",
    "TIM3",
    "TIM6",
    "TIM14",
    "TIM15",
    "TIM16",
    "TIM17",
    "I2C1",
    "I2C2",
    "SPI1",
    "SPI2",
    "USART1",
    "USART2",
    "USART3_4_5_6",
    "USB",
];

#[cfg(any(feature = "stm32f031", feature = "stm32f051",
          feature = "stm32f071", feature = "stm32f091"))]
const SUPPORTED_IRQS: [&str; 32] = [
    "WWDG",
    "PVD",
    "RTC",
    "FLASH",
    "RCC_CRS",
    "EXTI0_1",
    "EXTI2_3",
    "EXTI4_15",
    "TSC",
    "DMA1_CH1",
    "DMA1_CH2_3_DMA2_CH1_2",
    "DMA1_CH4_5_6_7_DMA2_CH3_4_5",
    "ADC_COMP",
    "TIM1_BRK_UP_TRG_COM",
    "TIM1_CC",
    "TIM2",
    "TIM3",
    "TIM6_DAC",
    "TIM7",
    "TIM14",
    "TIM15",
    "TIM16",
    "TIM17",
    "I2C1",
    "I2C2",
    "SPI1",
    "SPI2",
    "USART1",
    "USART2",
    "USART3_4_5_6_7_8",
    "CEC_CAN",
    "USB",
];

#[cfg(any(feature = "stm32f042", feature = "stm32f072"))]
const SUPPORTED_IRQS: [&str; 32] = [
    "WWDG",
    "PVD",
    "RTC",
    "FLASH",
    "RCC_CRS",
    "EXTI0_1",
    "EXTI2_3",
    "EXTI4_15",
    "TSC",
    "DMA1_CH1",
    "DMA1_CH2_3",
    "DMA1_CH4_5_6_7",
    "ADC_COMP",
    "TIM1_BRK_UP_TRG_COM",
    "TIM1_CC",
    "TIM2",
    "TIM3",
    "TIM6_DAC",
    "TIM7",
    "TIM14",
    "TIM15",
    "TIM16",
    "TIM17",
    "I2C1",
    "I2C2",
    "SPI1",
    "SPI2",
    "USART1",
    "USART2",
    "USART3_4",
    "CEC_CAN",
    "USB",
];

#[cfg(any(feature = "stm32f038", feature = "stm32f048", feature = "stm32f058",
          feature = "stm32f078", feature = "stm32f098"))]
const SUPPORTED_IRQS: [&str; 32] = [
    "WWDG",
    "PVD",
    "RTC",
    "FLASH",
    "RCC_CRS",
    "EXTI0_1",
    "EXTI2_3",
    "EXTI4_15",
    "TSC",
    "DMA1_CH1",
    "DMA1_CH2_3_DMA2_CH1_2",
    "DMA1_CH4_5_6_7_DMA2_CH3_4_5",
    "ADC_COMP",
    "TIM1_BRK_UP_TRG_COM",
    "TIM1_CC",
    "TIM2",
    "TIM3",
    "TIM6_DAC",
    "TIM7",
    "TIM14",
    "TIM15",
    "TIM16",
    "TIM17",
    "I2C1",
    "I2C2",
    "SPI1",
    "SPI2",
    "USART1",
    "USART2",
    "USART3_4_5_6_7_8",
    "CEC_CAN",
    "USB",
];
