// ============================================================
// main.js — Логика фронтенда vlessok
// ============================================================
// Обрабатывает нажатия кнопок, вызывает Rust-команды,
// обновляет UI (статус, лог, правила маршрутизации).
// ============================================================

// Получаем функции из Tauri
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const dialog = window.__TAURI__.plugin?.dialog || window.__TAURI__.dialog;

listen('singbox-error', (event) => {
  let msg = event.payload || '';
  // Убираем ANSI цветовые коды
  msg = msg.replace(/\x1B\[[0-9;]*[mG]/g, '');
  // Фильтруем некритичные ошибки (таймауты DNS, обычные обрывы отдельных соединений)
  const ignored = ['dns: exchange failed', 'i/o timeout', 'wsarecv:', 'aborted by the software', 'connection attempt failed', 'forcibly closed', 'connection download closed', 'operation not permitted'];
  if (ignored.some(err => msg.includes(err))) return;
  
  addLog(`❌ [SingBox]: ${msg}`, 'error');
});

// ============================================================
// Ссылки на DOM-элементы
// ============================================================
let profileSelect;
let btnAddProfile;
let btnEditProfile;
let btnDeleteProfile;
let profileModal;
let profileNameInput;
let profileUrlInput;
let btnSaveProfile;
let btnCancelProfile;

let btnConnect;
let btnDisconnect;
let statusDot;
let statusText;
let logOutput;
let btnClearLog;

let uacModal;
let btnRelaunchAdmin;
let btnCancelModal;
let connectionInfo;
let externalIpSpan;
let serverNameSpan;
let serverPingSpan;
let btnRefreshPing;
let btnResetNetwork;
let btnOpenConnections;

// Элементы маршрутизации
let routingGlobalRadio;
let routingRuleRadio;
let rulesSection;
let rulesEmptyWarning;
let tabBtns;
let tabContents;
let countDomains;
let countGeo;
let countProcesses;
let inputDomain;
let btnAddDomain;
let listDomains;
let presetGeoCheckboxes;
let inputGeo;
let btnAddGeo;
let listGeo;
let inputProcess;
let btnAddProcess;
let btnPickProcess;
let listProcesses;

let statusPollTimer = null;

// Глобальный стейт правил (синхронизируется с Rust)
let currentRules = {
  routing_mode: 'global',
  domains: [],
  geo_rules: [],
  processes: []
};

// Undo/Redo стэки
let undoStack = [];
let redoStack = [];

function saveHistoryState() {
  undoStack.push(JSON.parse(JSON.stringify(currentRules)));
  if (undoStack.length > 50) undoStack.shift();
  redoStack = [];
}

async function applyState(state) {
  try {
    await invoke('set_all_routing_rules', { newRules: state });
    await loadRoutingRules();
    addLog('🔙 Изменения отменены/повторены', 'info');
    await autoReconnect();
  } catch (err) {
    addLog(`❌ Ошибка отмены: ${err}`, 'error');
  }
}

async function handleUndo() {
  if (undoStack.length === 0) return;
  const prevState = undoStack.pop();
  redoStack.push(JSON.parse(JSON.stringify(currentRules)));
  await applyState(prevState);
}

async function handleRedo() {
  if (redoStack.length === 0) return;
  const nextState = redoStack.pop();
  undoStack.push(JSON.parse(JSON.stringify(currentRules)));
  await applyState(nextState);
}

window.addEventListener('keydown', (e) => {
  const isInput = ['INPUT', 'TEXTAREA'].includes(document.activeElement.tagName);

  if (e.key === 'Escape') {
    ['mass-import-modal', 'process-picker-modal', 'uac-modal'].forEach(id => {
      const m = document.getElementById(id);
      if (m) m.classList.add('hidden');
    });
    return;
  }

  if (e.ctrlKey && (e.key === 'z' || e.key === 'я')) {
    if (isInput) return; // Пусть работает браузерный Ctrl-Z для текста
    e.preventDefault();
    if (e.shiftKey) handleRedo();
    else handleUndo();
  } else if (e.ctrlKey && (e.key === 'y' || e.key === 'н')) {
    if (isInput) return;
    e.preventDefault();
    handleRedo();
  }
});

// ============================================================
// Управление профилями VLESS
// ============================================================
let profiles = [];
let editingProfileId = null;

function loadProfiles() {
  try {
    const data = localStorage.getItem('vlessok_profiles');
    if (data) profiles = JSON.parse(data);
  } catch(e) {}
  if (!Array.isArray(profiles)) profiles = [];
  renderProfiles();
}

function saveProfiles() {
  localStorage.setItem('vlessok_profiles', JSON.stringify(profiles));
}

function renderProfiles() {
  if (!profileSelect) return;
  const currentVal = profileSelect.value;
  profileSelect.innerHTML = '';
  
  if (profiles.length === 0) {
    const opt = document.createElement('option');
    opt.value = "";
    opt.textContent = "Нет профилей";
    profileSelect.appendChild(opt);
    if(btnConnect) btnConnect.disabled = true;
    if(btnEditProfile) btnEditProfile.disabled = true;
    if(btnDeleteProfile) btnDeleteProfile.disabled = true;
  } else {
    profiles.forEach(p => {
      const opt = document.createElement('option');
      opt.value = p.id;
      opt.textContent = p.name || 'Без имени';
      profileSelect.appendChild(opt);
    });
    // Восстанавливаем выбор
    let lastId = currentVal || localStorage.getItem('vlessok_last_profile_id');
    if (profiles.find(p => p.id === lastId)) {
      profileSelect.value = lastId;
    } else {
      profileSelect.value = profiles[0].id;
    }
    localStorage.setItem('vlessok_last_profile_id', profileSelect.value);
    
    if(btnConnect && statusText && statusText.textContent === 'ОТКЛЮЧЕНО') btnConnect.disabled = false;
    if(btnEditProfile) btnEditProfile.disabled = false;
    if(btnDeleteProfile) btnDeleteProfile.disabled = false;
  }
}

function openProfileModal(editId = null) {
  if (editId === null && profiles.length >= 5) {
    addLog('❌ Можно создать максимум 5 профилей', 'error');
    return;
  }
  editingProfileId = editId;
  const title = getEl('profile-modal-title');
  if (title) title.textContent = editId ? 'Редактировать профиль' : 'Новый профиль';
  
  if (editId) {
    const p = profiles.find(x => x.id === editId);
    if (p) {
      profileNameInput.value = p.name;
      profileUrlInput.value = p.url;
    }
  } else {
    profileNameInput.value = '';
    profileUrlInput.value = '';
  }
  if (profileModal) profileModal.classList.remove('hidden');
}

function closeProfileModal() {
  if (profileModal) profileModal.classList.add('hidden');
  editingProfileId = null;
}

function saveProfile() {
  const name = profileNameInput.value.trim();
  const url = profileUrlInput.value.trim();
  if (!url || !url.startsWith('vless://')) {
    addLog('❌ Ошибка: Укажите корректную vless:// ссылку', 'error');
    return;
  }
  
  if (editingProfileId) {
    const p = profiles.find(x => x.id === editingProfileId);
    if (p) {
      p.name = name || 'Без имени';
      p.url = url;
    }
  } else {
    const id = Date.now().toString();
    profiles.push({ id, name: name || 'Без имени', url });
    localStorage.setItem('vlessok_last_profile_id', id);
  }
  saveProfiles();
  renderProfiles();
  closeProfileModal();
}

function deleteProfile() {
  const id = profileSelect.value;
  if (!id) return;
  profiles = profiles.filter(p => p.id !== id);
  saveProfiles();
  renderProfiles();
}

function getSelectedVlessUrl() {
  const id = profileSelect.value;
  const p = profiles.find(x => x.id === id);
  return p ? p.url : null;
}

// ============================================================
// Вспомогательные функции
// ============================================================
function getEl(id) {
  const el = document.getElementById(id);
  if (!el) console.warn(`Элемент #${id} не найден в DOM!`);
  return el;
}

function bindEvent(el, event, handler) {
  if (el) el.addEventListener(event, handler);
}

// ============================================================
// Лог
// ============================================================
function addLog(message, type = 'info') {
  if (!logOutput) return;
  const now = new Date();
  const time = now.toTimeString().slice(0, 8);
  const entry = document.createElement('div');
  entry.className = `log-entry log-${type}`;
  entry.textContent = `[${time}] ${message}`;
  logOutput.appendChild(entry);
  logOutput.scrollTop = logOutput.scrollHeight;
}

// ============================================================
// Статус подключения
// ============================================================
function setConnected() {
  if (statusDot) statusDot.className = 'status-dot connected';
  if (statusText) {
    statusText.textContent = 'ПОДКЛЮЧЕНО';
    statusText.style.color = 'var(--connected-color)';
  }
  if (btnConnect) btnConnect.disabled = true;
  if (btnDisconnect) btnDisconnect.disabled = false;
}

function setDisconnected() {
  if (statusDot) statusDot.className = 'status-dot disconnected';
  if (statusText) {
    statusText.textContent = 'ОТКЛЮЧЕНО';
    statusText.style.color = 'var(--disconnected-color)';
  }
  if (btnConnect) btnConnect.disabled = false;
  if (btnDisconnect) btnDisconnect.disabled = true;
  if (connectionInfo) connectionInfo.classList.add('hidden');
}

function setConnecting() {
  if (statusDot) statusDot.className = 'status-dot connecting';
  if (statusText) {
    statusText.textContent = 'ПОДКЛЮЧЕНО';
    statusText.style.color = '#f39c12';
  }
  if (btnConnect) btnConnect.disabled = true;
  if (btnDisconnect) btnDisconnect.disabled = true;
}

async function pollStatus() {
  try {
    const running = await invoke('is_connected');
    if (running && statusText && statusText.textContent !== 'ПОДКЛЮЧЕНО') {
      setConnected();
    } else if (!running && statusText && statusText.textContent !== 'ОТКЛЮЧЕНО' && statusText.textContent !== 'ПОДКЛЮЧЕНИЕ...') {
      setDisconnected();
    }
  } catch (e) {
    console.warn('Ошибка опроса статуса:', e);
  }
}

// ============================================================
// Обработчики подключения
// ============================================================
async function doCheckPing(url) {
  if (!serverPingSpan) return;
  serverPingSpan.textContent = '...';
  try {
    const ping = await invoke('check_ping', { url });
    serverPingSpan.textContent = `${ping} мс`;
  } catch (err) {
    serverPingSpan.textContent = `Ошибка`;
  }
}

async function handleConnect() {
  const url = getSelectedVlessUrl();

  if (!url) {
    addLog('❌ Выберите или добавьте профиль VLESS', 'error');
    return;
  }
  if (!url.startsWith('vless://')) {
    addLog('❌ Ссылка должна начинаться с vless://', 'error');
    return;
  }

  const isAdmin = await invoke('is_admin');
  if (!isAdmin) {
    addLog('❌ Ошибка: нет прав администратора для режима TUN', 'error');
    if (uacModal) uacModal.classList.remove('hidden');
    return;
  }

  setConnecting();
  addLog('🌐 Создаём TUN-интерфейс...', 'info');

  try {
    const result = await invoke('connect_vless', { url });
    if (result === 'connected') {
      setConnected();
      
      addLog('✅ Системный VPN активен. Весь трафик идёт через VLESS-сервер.', 'success');
      const leakFixApplied = localStorage.getItem('dns_leak_fix_applied');
      if (!leakFixApplied) {
        addLog('🛡 Применяю защиту от DNS-leak...', 'info');
        try {
          await invoke('apply_dns_leak_fix');
          localStorage.setItem('dns_leak_fix_applied', 'true');
          addLog('✅ Защита от DNS-leak успешно применена', 'success');
        } catch (fixErr) {
          addLog(`❌ Ошибка применения DNS-leak: ${fixErr}`, 'warn');
        }
      }

      if (serverNameSpan) {
        try {
          serverNameSpan.textContent = new URL(url).hostname;
        } catch (e) {
          serverNameSpan.textContent = 'Неизвестно';
        }
      }

      if (externalIpSpan) externalIpSpan.textContent = "Определяем...";
      if (connectionInfo) connectionInfo.classList.remove('hidden');
      
      // Запускаем замер пинга в фоне
      doCheckPing(url);

      try {
        const ip = await invoke('get_current_external_ip');
        if (externalIpSpan) externalIpSpan.textContent = ip;
      } catch (err) {
        if (externalIpSpan) externalIpSpan.textContent = "Неизвестно";
      }
    }
  } catch (err) {
    setDisconnected();
    addLog(`❌ Ошибка подключения: ${err}`, 'error');
  }
}

async function handleDisconnect() {
  if (statusDot) statusDot.className = 'status-dot connecting';
  if (statusText) {
    statusText.textContent = 'ОТКЛЮЧЕНО';
    statusText.style.color = '#f39c12';
  }
  if (btnDisconnect) btnDisconnect.disabled = true;

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
// Маршрутизация (UI Логика)
// ============================================================
async function loadRoutingRules() {
  try {
    currentRules = await invoke('get_routing_rules');
    renderRoutingUI();
  } catch (err) {
    addLog(`❌ Ошибка загрузки правил: ${err}`, 'error');
  }
}

function renderRoutingUI() {
  if (!routingGlobalRadio || !routingRuleRadio || !rulesSection) return;

  const isRuleMode = currentRules.routing_mode === 'rule';
  if (isRuleMode) {
    routingRuleRadio.checked = true;
    rulesSection.classList.remove('hidden');
  } else {
    routingGlobalRadio.checked = true;
    rulesSection.classList.add('hidden');
  }

  // Обновляем каунтеры
  if (countDomains) countDomains.textContent = currentRules.domains.length;
  if (countGeo) countGeo.textContent = currentRules.geo_rules.length;
  if (countProcesses) countProcesses.textContent = currentRules.processes.length;

  // Если режим "Правило", но правил нет
  if (rulesEmptyWarning) {
    const totalRules = currentRules.domains.length + currentRules.geo_rules.length + currentRules.processes.length;
    if (isRuleMode && totalRules === 0) {
      rulesEmptyWarning.classList.remove('hidden');
    } else {
      rulesEmptyWarning.classList.add('hidden');
    }
  }

  // Рендер списков
  renderList(listDomains, currentRules.domains, 'remove_domain_rule');
  renderList(listGeo, currentRules.geo_rules, 'remove_geo_rule');
  renderList(listProcesses, currentRules.processes, 'remove_process_rule');

  // Обновление чекбоксов пресетов Geo
  if (presetGeoCheckboxes) {
    presetGeoCheckboxes.forEach(cb => {
      cb.checked = currentRules.geo_rules.includes(cb.value);
    });
  }
}

function renderList(container, items, removeCmd) {
  if (!container) return;
  container.innerHTML = '';
  items.forEach(item => {
    const div = document.createElement('div');
    div.className = 'rule-item';
    
    const span = document.createElement('span');
    span.className = 'rule-item-name';
    span.textContent = item;
    
    const btn = document.createElement('button');
    btn.className = 'rule-item-del';
    btn.innerHTML = '×';
    btn.onclick = async () => {
      saveHistoryState();
      try {
        let argName = removeCmd.split('_')[1];
        if (removeCmd === 'remove_geo_rule') {
          argName = 'rule';
        }
        await invoke(removeCmd, { [argName]: item });
        await loadRoutingRules(); // перезагружаем после удаления
        await autoReconnect();
      } catch (err) {
        addLog(`❌ Ошибка удаления: ${err}`, 'error');
        undoStack.pop();
      }
    };

    div.appendChild(span);
    div.appendChild(btn);
    container.appendChild(div);
  });
}

async function handleRoutingModeChange(e) {
  const newMode = e.target.value;
  try {
    await invoke('set_routing_mode', { mode: newMode });
    await loadRoutingRules();
    addLog(`🔄 Режим маршрутизации изменён на: ${newMode === 'rule' ? 'ПРАВИЛО' : 'ГЛОБАЛЬНО'}`, 'info');
    
    // Если подключены, нужно переподключиться
    await autoReconnect();
  } catch (err) {
    addLog(`❌ Ошибка смены режима маршрутизации: ${err}`, 'error');
  }
}

function switchTab(tabId) {
  if (tabBtns) {
    tabBtns.forEach(b => {
      if (b.dataset.tab === tabId) b.classList.add('active');
      else b.classList.remove('active');
    });
  }
  if (tabContents) {
    tabContents.forEach(c => {
      if (c.id === `tab-${tabId}`) c.classList.remove('hidden');
      else c.classList.add('hidden');
    });
  }
}

async function autoReconnect() {
  try {
    const isConnected = await invoke('is_connected');
    if (isConnected) {
      addLog('🔄 Применяем новые правила (перезапуск)...', 'info');
      await handleDisconnect();
      
      // Задержка 1.5 сек перед переподключением, чтобы ОС успела освободить TUN и маршруты
      await new Promise(resolve => setTimeout(resolve, 1500));
      
      await handleConnect();
    }
  } catch(e) {}
}

async function addMultipleRules(text, addCmd, argName) {
  const lines = text.split(/\r?\n/).map(l => l.trim()).filter(l => l);
  if (lines.length === 0) return;
  
  saveHistoryState();
  let addedCount = 0;
  for (const val of lines) {
    try {
      await invoke(addCmd, { [argName]: val });
      addedCount++;
    } catch (err) {
      if (err.includes('уже в списке')) {
        addLog(`⚠ Правило пропущено: ${err}`, 'warn');
      } else {
        console.warn(`Ошибка добавления ${val}:`, err);
      }
    }
  }
  
  if (addedCount > 0) {
    await loadRoutingRules();
    addLog(`✅ Добавлено новых правил: ${addedCount}`, 'success');
    await autoReconnect();
  } else {
    undoStack.pop(); // Ничего не добавлено
  }
}

function bindPasteEvent(inputEl, addCmd, argName) {
  if (!inputEl) return;
  inputEl.addEventListener('paste', (e) => {
    e.preventDefault();
    const text = (e.clipboardData || window.clipboardData).getData('text');
    inputEl.value = ''; // clear
    addMultipleRules(text, addCmd, argName);
  });
}

// Добавление правил
async function addRule(inputEl, addCmd, argName) {
  if (!inputEl) return;
  const val = inputEl.value.trim();
  if (!val) return;
  
  saveHistoryState();
  try {
    const added = await invoke(addCmd, { [argName]: val });
    inputEl.value = '';
    await loadRoutingRules();
    addLog(`✅ Добавлено правило: ${added}`, 'success');
    await autoReconnect();
  } catch (err) {
    undoStack.pop();
    if (err.includes('уже в списке')) {
      addLog(`⚠ ${err}`, 'warn');
    } else {
      addLog(`❌ Ошибка добавления: ${err}`, 'error');
    }
  }
}

async function handleGeoPresetChange(e) {
  const cb = e.target;
  const rule = cb.value;
  saveHistoryState();
  try {
    if (cb.checked) {
      await invoke('add_geo_rule', { rule });
    } else {
      await invoke('remove_geo_rule', { rule });
    }
    await loadRoutingRules();
    await autoReconnect();
  } catch (err) {
    undoStack.pop();
    cb.checked = !cb.checked; // откат UI
    addLog(`❌ Ошибка Geo пресета: ${err}`, 'error');
  }
}

async function pickProcessFile() {
  const modal = getEl('process-picker-modal');
  const list = getEl('process-list');
  if (!modal || !list) return;

  modal.classList.remove('hidden');
  list.innerHTML = '<div style="padding: 10px; color: var(--text-muted);">Загрузка процессов...</div>';
  
  try {
    const processes = await invoke('get_running_processes');
    list.innerHTML = '';
    
    // Группируем
    const apps = processes.filter(p => p.is_app && !currentRules.processes.some(cp => cp.toLowerCase() === p.name.toLowerCase()));
    const bg = processes.filter(p => !p.is_app && !currentRules.processes.some(cp => cp.toLowerCase() === p.name.toLowerCase()));

    const renderGroup = (title, items, isApp) => {
      if (items.length === 0) return;
      const h = document.createElement('h4');
      h.textContent = title;
      h.style.margin = '10px 0 5px 0';
      h.style.color = 'var(--text-muted)';
      list.appendChild(h);

      items.forEach(p => {
        const div = document.createElement('div');
        div.className = 'rule-item';
        div.style.cursor = 'pointer';
        
        const iconImg = document.createElement('img');
        iconImg.style.width = '16px';
        iconImg.style.height = '16px';
        iconImg.style.marginRight = '8px';
        // Пока ставим прозрачную заглушку, либо дефолтную иконку
        iconImg.src = isApp ? 'data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="transparent"/></svg>' : '';
        if (!isApp) {
          iconImg.style.display = 'none'; // для фоновых не показываем img, ставим emoji
        }
        iconImg.id = `icon-${p.name}`;

        const span = document.createElement('span');
        span.className = 'rule-item-name';
        if (isApp) {
          span.appendChild(iconImg);
          span.appendChild(document.createTextNode(p.name));
        } else {
          span.textContent = `⚙️ ${p.name}`;
        }
        
        div.appendChild(span);
        div.onclick = async () => {
          modal.classList.add('hidden');
          if (inputProcess) {
            inputProcess.value = p.name;
            await addRule(inputProcess, 'add_process_rule', 'process');
          }
        };
        list.appendChild(div);
      });
    };

    renderGroup('Приложения', apps, true);
    renderGroup('Фоновые процессы', bg, false);

    // Подгружаем иконки пакетом
    if (apps.length > 0) {
      const appNames = apps.map(p => p.name);
      invoke('get_process_icons_batched', { processNames: appNames })
        .then(iconMap => {
          for (const [name, b64] of Object.entries(iconMap)) {
            if (b64) {
              const img = document.getElementById(`icon-${name}`);
              if (img) img.src = b64;
            }
          }
        })
        .catch(console.error);
    }

    if (apps.length === 0 && bg.length === 0) {
      list.innerHTML = '<div style="padding: 10px; color: var(--text-muted);">Все активные процессы уже в правилах.</div>';
    }

    const searchInput = getEl('process-search');
    if (searchInput) {
      searchInput.value = '';
      searchInput.oninput = (e) => {
        const query = e.target.value.toLowerCase();
        Array.from(list.children).forEach(child => {
          if (child.tagName === 'H4') return;
          const txt = child.textContent.toLowerCase();
          child.style.display = txt.includes(query) ? 'flex' : 'none';
        });
      };
    }
  } catch (err) {
    list.innerHTML = `<div style="padding: 10px; color: var(--danger);">Ошибка: ${err}</div>`;
  }
}

async function clearAllRules(type) {
  const yes = await dialog.ask('Точно очистить все кастомные правила этой категории?', { title: 'Подтверждение очистки', type: 'warning' });
  if (!yes) return;
  saveHistoryState();
  try {
    const rulesToKeep = JSON.parse(JSON.stringify(currentRules));
    if (type === 'domain') rulesToKeep.domains = [];
    if (type === 'geo') rulesToKeep.geo_rules = [];
    if (type === 'process') rulesToKeep.processes = [];
    
    await applyState(rulesToKeep);
    addLog('✅ Список очищен', 'success');
  } catch(e) {
    undoStack.pop();
  }
}

let massImportType = '';
let massImportCmd = '';
let massImportArg = '';

function openMassImport(type, cmd, arg) {
  massImportType = type;
  massImportCmd = cmd;
  massImportArg = arg;
  const modal = getEl('mass-import-modal');
  const txt = getEl('mass-import-text');
  if (modal && txt) {
    txt.value = '';
    modal.classList.remove('hidden');
  }
}

// ============================================================
// Инициализация при загрузке страницы
// ============================================================
window.addEventListener('DOMContentLoaded', () => {
  try {
    // Находим элементы (оригинальные)
    profileSelect    = getEl('profile-select');
    btnAddProfile    = getEl('btn-add-profile');
    btnEditProfile   = getEl('btn-edit-profile');
    btnDeleteProfile = getEl('btn-delete-profile');
    profileModal     = getEl('profile-modal');
    profileNameInput = getEl('profile-name');
    profileUrlInput  = getEl('profile-url');
    btnSaveProfile   = getEl('btn-save-profile');
    btnCancelProfile = getEl('btn-cancel-profile');

    btnConnect       = getEl('btn-connect');
    btnDisconnect    = getEl('btn-disconnect');
    statusDot        = getEl('status-dot');
    statusText       = getEl('status-text');
    logOutput        = getEl('log-output');
    btnClearLog      = getEl('btn-clear-log');
    
    uacModal         = getEl('uac-modal');
    btnRelaunchAdmin = getEl('btn-relaunch-admin');
    btnCancelModal   = getEl('btn-cancel-modal');
    btnResetNetwork  = getEl('btn-reset-network');
    btnOpenConnections = getEl('btn-open-connections');
    connectionInfo   = getEl('connection-info');
    externalIpSpan   = getEl('external-ip');
    serverNameSpan   = getEl('server-name');
    serverPingSpan   = getEl('server-ping');
    btnRefreshPing   = getEl('btn-refresh-ping');
    btnResetNetwork  = getEl('btn-reset-network');

    // Находим элементы маршрутизации
    routingGlobalRadio = getEl('routing-global');
    routingRuleRadio   = getEl('routing-rule');
    rulesSection       = getEl('rules-section');
    rulesEmptyWarning  = getEl('rules-empty-warning');
    
    tabBtns      = document.querySelectorAll('.tab-btn');
    tabContents  = document.querySelectorAll('.tab-content');
    
    countDomains   = getEl('count-domains');
    countGeo       = getEl('count-geo');
    countProcesses = getEl('count-processes');
    
    inputDomain    = getEl('input-domain');
    btnAddDomain   = getEl('btn-add-domain');
    listDomains    = getEl('list-domains');
    
    presetGeoCheckboxes = document.querySelectorAll('.preset-geo');
    inputGeo            = getEl('input-geo');
    btnAddGeo           = getEl('btn-add-geo');
    listGeo             = getEl('list-geo');
    
    inputProcess   = getEl('input-process');
    btnAddProcess  = getEl('btn-add-process');
    btnPickProcess = getEl('btn-pick-process');
    listProcesses  = getEl('list-processes');

    // Профили
    loadProfiles();
    bindEvent(profileSelect, 'change', async () => {
      localStorage.setItem('vlessok_last_profile_id', profileSelect.value);
      const isConnected = await invoke('is_connected');
      if (isConnected) {
        await autoReconnect();
      }
    });
    bindEvent(btnAddProfile, 'click', () => openProfileModal(null));
    bindEvent(btnEditProfile, 'click', () => openProfileModal(profileSelect.value));
    bindEvent(btnDeleteProfile, 'click', async () => {
      const yes = await dialog.ask('Точно удалить этот профиль?', { title: 'Удаление', type: 'warning' });
      if (yes) deleteProfile();
    });
    bindEvent(btnCancelProfile, 'click', closeProfileModal);
    bindEvent(btnSaveProfile, 'click', saveProfile);
    if (btnRefreshPing) {
      bindEvent(btnRefreshPing, 'click', () => {
        const url = getSelectedVlessUrl();
        if (url) doCheckPing(url);
      });
    }

    // Автоматическое заполнение имени профиля при вставке ссылки
    if (profileUrlInput) {
      bindEvent(profileUrlInput, 'input', () => {
        const val = profileUrlInput.value.trim();
        if (val.startsWith('vless://') && !profileNameInput.value.trim()) {
          const idx = val.indexOf('#');
          if (idx !== -1) {
            let name = val.substring(idx + 1).trim();
            try { name = decodeURIComponent(name); } catch(e) {}
            if (name) profileNameInput.value = name;
          }
        }
      });
    }

    // Режим TUN теперь включен всегда, восстанавливать нечего

    // Привязка кнопок (оригинальные)
    bindEvent(btnConnect, 'click', handleConnect);
    bindEvent(btnDisconnect, 'click', handleDisconnect);
    bindEvent(btnCancelModal, 'click', () => {
      if (uacModal) uacModal.classList.add('hidden');
      addLog('⚠ Запуск отменён пользователем', 'warn');
    });
    bindEvent(btnRelaunchAdmin, 'click', async () => {
      if (uacModal) uacModal.classList.add('hidden');
      addLog('🔐 Запрашиваем права администратора...', 'info');
      try { await invoke('relaunch_as_admin'); } 
      catch (e) { addLog(`❌ Ошибка: ${e}`, 'error'); }
    });
    bindEvent(btnResetNetwork, 'click', async () => {
      const yes = await dialog.ask('Вы уверены, что хотите сбросить настройки сети? Это может временно прервать соединения.', { title: 'Сброс сети', type: 'warning' });
      if (!yes) return;
      addLog('🔄 Сбрасываем сетевые настройки...', 'info');
      try {
        const res = await invoke('reset_network');
        localStorage.removeItem('dns_leak_fix_applied');
        addLog(`✅ ${res}`, 'success');
      } catch (e) { addLog(`❌ Ошибка сброса сети: ${e}`, 'error'); }
    });

    bindEvent(btnOpenConnections, 'click', async () => {
      try {
        await invoke('open_connections_window');
      } catch (err) {
        addLog(`❌ Ошибка открытия окна соединений: ${err}`, 'error');
      }
    });
    bindEvent(btnClearLog, 'click', () => {
      if (logOutput) logOutput.innerHTML = '';
      addLog('Лог очищен', 'info');
    });

    // Привязка событий маршрутизации
    bindEvent(routingGlobalRadio, 'change', handleRoutingModeChange);
    bindEvent(routingRuleRadio, 'change', handleRoutingModeChange);

    if (tabBtns) {
      tabBtns.forEach(btn => {
        bindEvent(btn, 'click', () => switchTab(btn.dataset.tab));
      });
    }

    bindEvent(btnAddDomain, 'click', () => addRule(inputDomain, 'add_domain_rule', 'domain'));
    if (inputDomain) bindEvent(inputDomain, 'keypress', (e) => { if (e.key === 'Enter') btnAddDomain.click(); });
    bindPasteEvent(inputDomain, 'add_domain_rule', 'domain');

    bindEvent(btnAddGeo, 'click', () => addRule(inputGeo, 'add_geo_rule', 'rule'));
    if (inputGeo) bindEvent(inputGeo, 'keypress', (e) => { if (e.key === 'Enter') btnAddGeo.click(); });
    bindPasteEvent(inputGeo, 'add_geo_rule', 'rule');

    if (presetGeoCheckboxes) {
      presetGeoCheckboxes.forEach(cb => bindEvent(cb, 'change', handleGeoPresetChange));
    }

    bindEvent(btnAddProcess, 'click', () => addRule(inputProcess, 'add_process_rule', 'process'));
    if (inputProcess) bindEvent(inputProcess, 'keypress', (e) => { if (e.key === 'Enter') btnAddProcess.click(); });
    bindPasteEvent(inputProcess, 'add_process_rule', 'process');
    bindEvent(btnPickProcess, 'click', pickProcessFile);

    bindEvent(getEl('btn-clear-domains'), 'click', () => clearAllRules('domain'));
    bindEvent(getEl('btn-clear-geo'), 'click', () => clearAllRules('geo'));
    bindEvent(getEl('btn-clear-processes'), 'click', () => clearAllRules('process'));

    bindEvent(getEl('btn-mass-domain'), 'click', () => openMassImport('domain', 'add_domain_rule', 'domain'));
    bindEvent(getEl('btn-mass-geo'), 'click', () => openMassImport('geo', 'add_geo_rule', 'rule'));
    bindEvent(getEl('btn-mass-process'), 'click', () => openMassImport('process', 'add_process_rule', 'process'));

    bindEvent(getEl('btn-mass-import-cancel'), 'click', () => { getEl('mass-import-modal').classList.add('hidden'); });
    bindEvent(getEl('btn-process-picker-cancel'), 'click', () => { getEl('process-picker-modal').classList.add('hidden'); });

    bindEvent(getEl('btn-mass-import-apply'), 'click', async () => {
      const txt = getEl('mass-import-text').value;
      getEl('mass-import-modal').classList.add('hidden');
      if (txt) {
        await addMultipleRules(txt, massImportCmd, massImportArg);
      }
    });

    // Запускаем опрос статуса каждую секунду
    pollStatus();
    statusPollTimer = setInterval(pollStatus, 1000);

    // Загружаем правила маршрутизации
    loadRoutingRules();

    addLog('Приложение готово. Вставьте VLESS-ссылку и нажмите Подключить.', 'info');
  } catch (err) {
    console.error('Критическая ошибка при инициализации UI:', err);
    if (logOutput) addLog(`❌ Ошибка UI: ${err.message}`, 'error');
  }
});
