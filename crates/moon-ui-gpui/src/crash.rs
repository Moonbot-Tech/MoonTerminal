//! Нативный обработчик крашей (Windows SEH). Дополняет Rust-паник-хук из `main`:
//! паник-хук ловит ТОЛЬКО Rust-паники (unwind), а нативные access violation в
//! DirectX/GPUI-форке (например, present по протухшему дескриптору окна при
//! реконнект-шторме) идут мимо него — процесс просто умирает, лог обрывается, и
//! `panic.log` пуст. Этот фильтр верхнего уровня перехватывает такие исключения,
//! пишет код/адрес сбоя и бэктрейс ВЫЗЫВАЮЩЕГО (упавшего) потока в `panic.log` —
//! ровно туда же, куда пишет паник-хук, — чтобы краш был виден в одном месте.

/// Ставит top-level фильтр исключений процесса. Вызывать один раз на старте, как
/// можно раньше (после установки cwd, до создания окон). На не-Windows — no-op.
pub fn install_native_handler() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter;
        SetUnhandledExceptionFilter(Some(native_exception_filter));
    }
}

#[cfg(windows)]
unsafe extern "system" fn native_exception_filter(
    info: *const windows::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
    use std::io::Write;
    // EXCEPTION_CONTINUE_SEARCH=0: продолжаем штатную обработку (WER/abort) после лога,
    // не подменяя поведение завершения процесса.
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    const STATUS_ACCESS_VIOLATION: u32 = 0xC0000005;

    // Реентрант-гард: если фильтр сам упадёт (или повторное исключение во время
    // логирования), не зацикливаемся.
    static IN_HANDLER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if IN_HANDLER.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return EXCEPTION_CONTINUE_SEARCH;
    }

    let mut body = String::new();
    if let Some(info) = unsafe { info.as_ref() } {
        if let Some(rec) = unsafe { info.ExceptionRecord.as_ref() } {
            let code = rec.ExceptionCode.0 as u32;
            let addr = rec.ExceptionAddress as usize;
            body.push_str(&format!("code=0x{code:08X} at instruction 0x{addr:016X}"));
            // Для access violation первые два параметра — тип (0=чтение,1=запись,8=DEP)
            // и адрес недоступной памяти.
            if code == STATUS_ACCESS_VIOLATION && rec.NumberParameters >= 2 {
                let kind = match rec.ExceptionInformation[0] {
                    0 => "read",
                    1 => "write",
                    8 => "execute(DEP)",
                    _ => "?",
                };
                let fault = rec.ExceptionInformation[1];
                body.push_str(&format!(" — access violation ({kind}) at 0x{fault:016X}"));
            }
        } else {
            body.push_str("<no exception record>");
        }
    } else {
        body.push_str("<no exception pointers>");
    }

    // Бэктрейс упавшего потока: фильтр исполняется НА том же потоке, поэтому
    // force_capture даёт стек сбоя (символизуется по нашим PDB, как в паник-хуке).
    let bt = std::backtrace::Backtrace::force_capture();
    let line = format!("NATIVE CRASH: {body}\n--- backtrace ---\n{bt}\n--- end ---");

    // Только прямой файловый IO: глобальный логгер мог держать lock на упавшем потоке
    // (риск дедлока). `panic.log` — тот же файл, что у Rust-паник-хука.
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("panic.log")
    {
        let _ = writeln!(f, "{line}");
        let _ = f.flush();
    }

    EXCEPTION_CONTINUE_SEARCH
}
