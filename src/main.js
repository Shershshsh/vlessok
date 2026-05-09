// ============================================================
// main.js — Логика фронтенда vlessok (Этап 2)
// ============================================================
// Обрабатывает нажатия кнопок, вызывает Rust-команды,
// обновляет UI (статус, лог).
// ============================================================

// Получаем функцию invoke из Tauri для вызова Rust-команд
const { invoke } = window.__TAURI__.core;

// ============================================================
// Ссылки на DOM-элементы (находим при загрузке страницы)
// ============================================================
let vlessUrlInput;   // textarea для VLESS-ссылки
let btnConnect;      // кнопка "Подключить"
let btnDisconnect;   // кнопка "Отключить"
let statusDot;       // цветной кружок статуса
let statusText;      // текст статуса ("Подключено" / "Отключено")
let logOutput;       // контейнер лога
let btnClearLog;     // кнопка "Очистить" лог

// ID таймера опроса статуса (нужен для отмены)
let statusPollTimer = null;

// ============================================================
// Функции для работы с логом
// ============================================================

/**
 * Добавляет запись в лог.
 * @param {string} message - текст сообщения
 * @param {'info'|'success'|'error'|'warn'} type - тип записи
 */
function addLog(message, type = 'info') {
  const now = new Date();
  // Формат времени: ЧЧ:ММ:СС
  const time = now.toTimeString().slice(0, 8);

  const entry = document.createElement('div');
  entry.className = `log-entry log-${type}`;
  entry.textContent = `[${time}] ${message}`;

  logOutput.appendChild(entry);

  // Автоскролл вниз чтобы видеть последнюю запись
  logOutput.scrollTop = logOutput.scrollHeight;
}

// ============================================================
// Функции для обновления статуса
// ============================================================

/**
 * Устанавливает UI в состояние "Подключено".
 */
function setConnected() {
  statusDot.className = 'status-dot connected';
  statusText.textContent = 'Подключено';
  statusText.style.color = 'var(--connected-color)';
  btnConnect.disabled = true;
  btnDisconnect.disabled = false;
}

/**
 * Устанавливает UI в состояние "Отключено".
 */
function setDisconnected() {
  statusDot.className = 'status-dot disconnected';
  statusText.textContent = 'Отключено';
  statusText.style.color = 'var(--disconnected-color)';
  btnConnect.disabled = false;
  btnDisconnect.disabled = true;
}

/**
 * Устанавливает UI в состояние "Подключение...".
 */
function setConnecting() {
  statusDot.className = 'status-dot connecting';
  statusText.textContent = 'Подключение...';
  statusText.style.color = '#f39c12';
  btnConnect.disabled = true;
  btnDisconnect.disabled = true;
}

// ============================================================
// Опрос статуса (раз в секунду)
// ============================================================

/**
 * Запрашивает у бэкенда актуальный статус подключения
 * и обновляет UI. Запускается раз в секунду.
 */
async function pollStatus() {
  try {
    const running = await invoke('is_connected');
    // Обновляем UI только если кнопки не в состоянии "connecting"
    // (чтобы не сбрасывать промежуточное состояние)
    if (statusText.textContent !== 'Подключение...' &&
        statusText.textContent !== 'Отключение...') {
      if (running) {
        setConnected();
      } else {
        setDisconnected();
      }
    }
  } catch (e) {
    // Ошибка опроса — не страшно, просто пишем в лог (редко)
    console.warn('Ошибка опроса статуса:', e);
  }
}

// ============================================================
// Обработчики кнопок
// ============================================================

/**
 * Нажатие "Подключить": берёт VLESS URL из textarea,
 * вызывает connect_vless на бэкенде.
 */
async function handleConnect() {
  const url = vlessUrlInput.value.trim();

  // Проверяем что поле не пустое
  if (!url) {
    addLog('❌ Вставьте VLESS-ссылку в поле выше', 'error');
    return;
  }

  // Базовая проверка схемы прямо на фронтенде (для быстрого фидбека)
  if (!url.startsWith('vless://')) {
    addLog('❌ Ссылка должна начинаться с vless://', 'error');
    return;
  }

  setConnecting();
  addLog('⏳ Запуск sing-box...', 'info');

  try {
    // Вызываем Rust-команду connect_vless с VLESS URL
    const result = await invoke('connect_vless', { url });

    if (result === 'connected') {
      setConnected();
      addLog('✅ sing-box запущен. Прокси доступен на 127.0.0.1:10808', 'success');
      addLog('📡 Для теста: curl --socks5 127.0.0.1:10808 https://ifconfig.me', 'info');
    }
  } catch (err) {
    // Rust вернул ошибку — показываем её
    setDisconnected();
    addLog(`❌ Ошибка подключения: ${err}`, 'error');
  }
}

/**
 * Нажатие "Отключить": вызывает disconnect на бэкенде.
 */
async function handleDisconnect() {
  statusDot.className = 'status-dot connecting';
  statusText.textContent = 'Отключение...';
  statusText.style.color = '#f39c12';
  btnDisconnect.disabled = true;

  addLog('⏳ Останавливаем sing-box...', 'info');

  try {
    await invoke('disconnect');
    setDisconnected();
    addLog('✅ sing-box остановлен. Подключение закрыто.', 'success');
  } catch (err) {
    setDisconnected();
    addLog(`❌ Ошибка при отключении: ${err}`, 'error');
  }
}

// ============================================================
// Инициализация при загрузке страницы
// ============================================================

window.addEventListener('DOMContentLoaded', () => {
  // Находим все элементы
  vlessUrlInput  = document.querySelector('#vless-url');
  btnConnect     = document.querySelector('#btn-connect');
  btnDisconnect  = document.querySelector('#btn-disconnect');
  statusDot      = document.querySelector('#status-dot');
  statusText     = document.querySelector('#status-text');
  logOutput      = document.querySelector('#log-output');
  btnClearLog    = document.querySelector('#btn-clear-log');

  // Обработчики кнопок
  btnConnect.addEventListener('click', handleConnect);
  btnDisconnect.addEventListener('click', handleDisconnect);

  // Кнопка очистки лога
  btnClearLog.addEventListener('click', () => {
    logOutput.innerHTML = '';
    addLog('Лог очищен', 'info');
  });

  // Запускаем опрос статуса каждую секунду
  pollStatus(); // сразу при запуске
  statusPollTimer = setInterval(pollStatus, 1000);

  addLog('Приложение готово. Вставьте VLESS-ссылку и нажмите «Подключить».', 'info');
});
