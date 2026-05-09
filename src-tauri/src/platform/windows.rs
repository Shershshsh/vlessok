// ============================================================
// platform/windows.rs — Windows API утилиты
// ============================================================

use std::os::windows::io::AsRawHandle;
use std::process::Child;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
    JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

/// Обёртка над Windows Job Object.
/// При удалении объекта (Drop) или смерти родительского процесса,
/// все процессы, привязанные к этому Job Object, будут автоматически убиты ОС.
pub struct ProcessJob {
    handle: HANDLE,
}

// HANDLE в Windows — это просто число/указатель, безопасный для передачи между потоками
unsafe impl Send for ProcessJob {}
unsafe impl Sync for ProcessJob {}

impl ProcessJob {
    /// Создаёт новый Job Object и настраивает флаг KILL_ON_JOB_CLOSE.
    pub fn new() -> Result<Self, String> {
        unsafe {
            // Создаём безымянный Job Object
            let job_handle = CreateJobObjectW(None, None)
                .map_err(|e| format!("Ошибка CreateJobObjectW: {}", e))?;

            // Если вернулся невалидный хэндл
            if job_handle.is_invalid() {
                return Err("CreateJobObjectW вернул инвалидный хэндл".to_string());
            }

            // Настраиваем флаг ограничения
            let mut limit_info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limit_info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            // Применяем настройки к Job Object
            let result = SetInformationJobObject(
                job_handle,
                JobObjectExtendedLimitInformation,
                &limit_info as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );

            if result.is_err() {
                let _ = CloseHandle(job_handle);
                return Err(format!("Ошибка SetInformationJobObject: {}", result.unwrap_err()));
            }

            Ok(ProcessJob { handle: job_handle })
        }
    }

    /// Привязывает дочерний процесс к этому Job Object.
    /// Важно вызывать СРАЗУ после spawn() дочернего процесса.
    pub fn assign_process(&self, child: &Child) -> Result<(), String> {
        unsafe {
            // Получаем "сырой" хэндл процесса (Child)
            let raw_handle = child.as_raw_handle();
            let process_handle = HANDLE(raw_handle as _);

            // Привязываем процесс к Job Object
            let result = AssignProcessToJobObject(self.handle, process_handle);

            if result.is_err() {
                return Err(format!("Ошибка AssignProcessToJobObject: {}", result.unwrap_err()));
            }

            Ok(())
        }
    }
}

/// Гарантированное закрытие хэндла Job Object при уничтожении структуры.
/// Хотя при смерти нашего приложения Windows сама всё закроет, хороший тон убирать за собой.
impl Drop for ProcessJob {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

/// Проверка прав администратора для текущего процесса.
pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION,
        TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcessToken,
    };

    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = 0u32;
        let result = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );
        let _ = CloseHandle(token);
        result.is_ok() && elevation.TokenIsElevated != 0
    }
}
// 
// // Оставим эти функции закомментированными для справки:
// // Альтернативный метод привязки ТЕКУЩЕГО процесса к Job Object (как предлагалось изначально).
// // Мы отказались от него в пользу привязки дочернего процесса (AssignProcessToJobObject(child)),
// // так как привязка родителя может конфликтовать с внутренними механизмами Tauri/WebView2.
