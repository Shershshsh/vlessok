// ============================================================
// main.js — Главный JavaScript-файл фронтенда vlessok
// ============================================================
// Здесь находится логика взаимодействия UI с Rust-бэкендом.
// Для вызова Rust-команд используется window.__TAURI__.core.invoke()
// ============================================================

// Получаем функцию invoke из Tauri — она позволяет вызывать Rust-команды
const { invoke } = window.__TAURI__.core;

// Ждём полной загрузки HTML-страницы, прежде чем работать с элементами
window.addEventListener("DOMContentLoaded", () => {
  // Находим нужные элементы на странице
  const testBtn = document.querySelector("#test-btn");
  const resultMsg = document.querySelector("#result-msg");

  // Обработчик нажатия кнопки "Проверить связь с бэкендом"
  testBtn.addEventListener("click", async () => {
    try {
      // Вызываем Rust-команду test_backend (определена в lib.rs)
      // invoke() возвращает Promise — результат придёт асинхронно
      const response = await invoke("test_backend");

      // Показываем ответ от Rust в зелёном блоке
      resultMsg.textContent = response;
      resultMsg.className = "result-msg success";
    } catch (error) {
      // Если что-то пошло не так — показываем ошибку в красном блоке
      resultMsg.textContent = "❌ Ошибка: " + error;
      resultMsg.className = "result-msg error";
    }
  });
});
