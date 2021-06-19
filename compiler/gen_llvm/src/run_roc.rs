use std::ffi::CString;
use std::os::raw::c_char;
use RocCallResult::*;

/// This must have the same size as the repr() of RocCallResult!
pub const ROC_CALL_RESULT_DISCRIMINANT_SIZE: usize = std::mem::size_of::<u64>();

#[repr(u64)]
pub enum RocCallResult<T> {
    Success(T),
    Failure(*mut c_char),
}

impl<T: Sized> From<RocCallResult<T>> for Result<T, String> {
    fn from(call_result: RocCallResult<T>) -> Self {
        match call_result {
            Success(value) => Ok(value),
            Failure(failure) => Err({
                let raw = unsafe { CString::from_raw(failure) };

                let result = format!("{:?}", raw);

                // make sure rust does not try to free the Roc string
                std::mem::forget(raw);

                result
            }),
        }
    }
}

#[macro_export]
macro_rules! run_jit_function {
    ($lib: expr, $main_fn_name: expr, $ty:ty, $transform:expr) => {{
        let v: String = String::new();
        run_jit_function!($lib, $main_fn_name, $ty, $transform, v)
    }};

    ($lib: expr, $main_fn_name: expr, $ty:ty, $transform:expr, $errors:expr) => {{
        use inkwell::context::Context;
        use roc_gen_llvm::run_roc::RocCallResult;
        use std::mem::MaybeUninit;

        unsafe {
            let main: libloading::Symbol<unsafe extern "C" fn(*mut RocCallResult<$ty>) -> ()> =
                $lib.get($main_fn_name.as_bytes())
                    .ok()
                    .ok_or(format!("Unable to JIT compile `{}`", $main_fn_name))
                    .expect("errored");

            let mut result = MaybeUninit::uninit();

            main(result.as_mut_ptr());

            match result.assume_init().into() {
                Ok(success) => {
                    // only if there are no exceptions thrown, check for errors
                    assert!($errors.is_empty(), "Encountered errors:\n{}", $errors);

                    $transform(success)
                }
                Err(error_msg) => panic!("Roc failed with message: {}", error_msg),
            }
        }
    }};
}

/// In the repl, we don't know the type that is returned; it it's large enough to not fit in 2
/// registers (i.e. size bigger than 16 bytes on 64-bit systems), then we use this macro.
/// It explicitly allocates a buffer that the roc main function can write its result into.
#[macro_export]
macro_rules! run_jit_function_dynamic_type {
    ($lib: expr, $main_fn_name: expr, $bytes:expr, $transform:expr) => {{
        let v: String = String::new();
        run_jit_function_dynamic_type!($lib, $main_fn_name, $bytes, $transform, v)
    }};

    ($lib: expr, $main_fn_name: expr, $bytes:expr, $transform:expr, $errors:expr) => {{
        use inkwell::context::Context;
        use roc_gen_llvm::run_roc::RocCallResult;

        unsafe {
            let main: libloading::Symbol<unsafe extern "C" fn(*const u8)> = $lib
                .get($main_fn_name.as_bytes())
                .ok()
                .ok_or(format!("Unable to JIT compile `{}`", $main_fn_name))
                .expect("errored");

            let size = roc_gen_llvm::run_roc::ROC_CALL_RESULT_DISCRIMINANT_SIZE + $bytes;
            let layout = std::alloc::Layout::array::<u8>(size).unwrap();
            let result = std::alloc::alloc(layout);
            main(result);

            let flag = *result;

            if flag == 0 {
                $transform(result.offset(8) as *const u8)
            } else {
                use std::ffi::CString;
                use std::os::raw::c_char;

                // first field is a char pointer (to the error message)
                // read value, and transmute to a pointer
                let ptr_as_int = *(result as *const u64).offset(1);
                let ptr = std::mem::transmute::<u64, *mut c_char>(ptr_as_int);

                // make CString (null-terminated)
                let raw = CString::from_raw(ptr);

                let result = format!("{:?}", raw);

                // make sure rust doesn't try to free the Roc constant string
                std::mem::forget(raw);

                eprintln!("{}", result);
                panic!("Roc hit an error");
            }
        }
    }};
}