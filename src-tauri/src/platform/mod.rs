// ============================================================
// platform/mod.rs — Кроссплатформенные утилиты
// ============================================================

#[cfg(target_os = "windows")]
pub mod windows;

// Абстракция (заглушка) для других ОС, если потребуется сборка не под Windows
#[cfg(not(target_os = "windows"))]
pub mod windows {
    use std::process::Child;

    // Пустая заглушка Job Object для не-Windows систем
    pub struct ProcessJob;

    impl ProcessJob {
        pub fn new() -> Result<Self, String> {
            Ok(Self)
        }
        pub fn assign_process(&self, _child: &Child) -> Result<(), String> {
            Ok(())
        }
    }
}
