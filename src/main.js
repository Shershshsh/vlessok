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

// Элементы Этапа 3
let modeTunRadio;
let modeSocksRadio;
let uacModal;
let btnRelaunchAdmin;
let btnUseSocks;
let btnCancelModal;
let connectionInfo;
let externalIpSpan;
let serverNameSpan;
let btnResetNetwork;

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
  
  // Отключаем переключение режима во время работы
  modeTunRadio.disabled = true;
  modeSocksRadio.disabled = true;
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
  
  // Включаем переключение режима
  modeTunRadio.disabled = false;
  modeSocksRadio.disabled = false;
  connectionInfo.classList.add('hidden');
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

  // Базовая проверка схемы
  if (!url.startsWith('vless://')) {
    addLog('❌ Ссылка должна начинаться с vless://', 'error');
    return;
  }

  const mode = modeTunRadio.checked ? 'tun' : 'socks';

  // Проверка прав администратора для TUN
  if (mode === 'tun') {
    const isAdmin = await invoke('is_admin');
    if (!isAdmin) {
      addLog('❌ Ошибка: нет прав администратора для режима TUN', 'error');
      uacModal.classList.remove('hidden');
      return;
    }
  }

  setConnecting();
  if (mode === 'tun') {
    addLog('🌐 Создаём TUN-интерфейс...', 'info');
  } else {
    addLog('⏳ Запуск sing-box...', 'info');
  }

  try {
    // Вызываем Rust-команду connect_vless
    const result = await invoke('connect_vless', { url, mode });

    if (result === 'connected') {
      setConnected();
      
      if (mode === 'tun') {
        addLog('✅ Системный VPN активен. Весь трафик идёт через VLESS-сервер.', 'success');
        
        // Применяем защиту от DNS leak при первом запуске
        const leakFixApplied = localStorage.getItem('dns_leak_fix_applied');
        if (!leakFixApplied) {
          addLog('🛡 Применяю защиту от DNS-leak (требует прав админа)...', 'info');
          try {
            await invoke('apply_dns_leak_fix');
            localStorage.setItem('dns_leak_fix_applied', 'true');
            addLog('✅ Защита от DNS-leak успешно применена', 'success');
          } catch (fixErr) {
            addLog(`❌ Ошибка применения DNS-leak: ${fixErr}`, 'warn');
          }
        }
      } else {
        addLog('✅ sing-box запущен. Прокси доступен на 127.0.0.1:10808', 'success');
      }

      // Извлекаем хост сервера из URL для UI
      try {
        const parsedUrl = new URL(url);
        serverNameSpan.textContent = parsedUrl.hostname;
      } catch (e) {
        serverNameSpan.textContent = 'Неизвестно';
      }

      // Показываем блок инфо (с "Загрузка..." в IP)
      externalIpSpan.textContent = "Определяем...";
      connectionInfo.classList.remove('hidden');

      // Опрашиваем реальный IP
      try {
        const ip = await invoke('get_current_external_ip');
        externalIpSpan.textContent = ip;
      } catch (err) {
        externalIpSpan.textContent = "Неизвестно";
      }
    }
  } catch (err) {
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
  try {
    // Находим все элементы
    vlessUrlInput  = document.querySelector('#vless-url');
    btnConnect     = document.querySelector('#btn-connect');
    btnDisconnect  = document.querySelector('#btn-disconnect');
    statusDot      = document.querySelector('#status-dot');
    statusText     = document.querySelector('#status-text');
    logOutput      = document.querySelector('#log-output');
    btnClearLog    = document.querySelector('#btn-clear-log');
    
    modeTunRadio   = document.querySelector('#mode-tun');
    modeSocksRadio = document.querySelector('#mode-socks');
    uacModal       = document.querySelector('#uac-modal');
    btnRelaunchAdmin = document.querySelector('#btn-relaunch-admin');
    btnUseSocks    = document.querySelector('#btn-use-socks');
    btnCancelModal = document.querySelector('#btn-cancel-modal');
    connectionInfo = document.querySelector('#connection-info');
    externalIpSpan = document.querySelector('#external-ip');
    serverNameSpan = document.querySelector('#server-name');
    btnResetNetwork = document.querySelector('#btn-reset-network');

    // Проверяем критичные элементы
    if (!modeTunRadio || !modeSocksRadio) {
      console.error('Ошибка инициализации: Не найдены элементы переключателя режима (#mode-tun или #mode-socks).');
      addLog('❌ Критическая ошибка UI: не найдены элементы выбора режима.', 'error');
    } else {
      // Восстанавливаем сохраненный режим
      const savedMode = localStorage.getItem('vpn_mode');
      if (savedMode === 'socks') {
        modeSocksRadio.checked = true;
      } else {
        modeTunRadio.checked = true;
      }

      // Сохраняем режим при изменении
      modeTunRadio.addEventListener('change', () => localStorage.setItem('vpn_mode', 'tun'));
      modeSocksRadio.addEventListener('change', () => localStorage.setItem('vpn_mode', 'socks'));
    }

    // Обработчики кнопок
    if (btnConnect) btnConnect.addEventListener('click', handleConnect);
    if (btnDisconnect) btnDisconnect.addEventListener('click', handleDisconnect);

    // Обработчики модального окна
    if (btnCancelModal) btnCancelModal.addEventListener('click', () => {
      if (uacModal) uacModal.classList.add('hidden');
      addLog('⚠ Запуск отменён пользователем', 'warn');
    });

    if (btnUseSocks) btnUseSocks.addEventListener('click', () => {
      if (uacModal) uacModal.classList.add('hidden');
      if (modeSocksRadio) modeSocksRadio.checked = true;
      localStorage.setItem('vpn_mode', 'socks');
      handleConnect();
    });

    if (btnRelaunchAdmin) btnRelaunchAdmin.addEventListener('click', async () => {
      if (uacModal) uacModal.classList.add('hidden');
      addLog('🔐 Запрашиваем права администратора...', 'info');
      try {
        await invoke('relaunch_as_admin');
      } catch (e) {
        addLog(`❌ Ошибка: ${e}`, 'error');
      }
    });

    // Кнопка сброса сети
    if (btnResetNetwork) btnResetNetwork.addEventListener('click', async () => {
      addLog('🔄 Сбрасываем сетевые настройки...', 'info');
      try {
        const res = await invoke('reset_network');
        localStorage.removeItem('dns_leak_fix_applied'); // Сбрасываем флаг, чтобы применить заново при нужном подключении
        addLog(`✅ ${res}`, 'success');
      } catch (e) {
        addLog(`❌ Ошибка сброса сети: ${e}`, 'error');
      }
    });

    // Кнопка очистки лога
    if (btnClearLog) btnClearLog.addEventListener('click', () => {
      if (logOutput) logOutput.innerHTML = '';
      addLog('Лог очищен', 'info');
    });

    // Запускаем опрос статуса каждую секунду
    pollStatus(); // сразу при запуске
    statusPollTimer = setInterval(pollStatus, 1000);

    addLog('Приложение готово. Вставьте VLESS-ссылку и выберите режим работы.', 'info');
  } catch (err) {
    console.error('Критическая ошибка при инициализации UI:', err);
    if (logOutput) {
      addLog(`❌ Ошибка UI: ${err.message}`, 'error');
    }
  }
});
