// ============================================================
// connections.js — Логика окна "Монитор соединений" v2.1
// ============================================================

let ws = null;
let currentConnections = [];
// Состояния сортировки: 'desc', 'asc', 'none'
let sortState = {
  time: 'desc',
  traffic: 'none',
  process: 'none'
};
let currentSort = 'time';
const globalUniqueProcesses = new Set();

const listContainer = document.getElementById('connections-list');
const filterProcess = document.getElementById('filter-process');
const processDatalist = document.getElementById('process-list-datalist');
const filterDomain = document.getElementById('filter-domain');
const filterRoute = document.getElementById('filter-route');
const activeCountSpan = document.getElementById('active-count');
const sortBtns = document.querySelectorAll('.sort-btn');

function formatBytes(bytes) {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
}

function formatTime(startTimeStr) {
  const start = new Date(startTimeStr);
  const diff = Math.floor((Date.now() - start.getTime()) / 1000);
  if (diff < 60) return `${diff}s`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ${diff % 60}s`;
  return `${Math.floor(diff / 3600)}h ${Math.floor((diff % 3600) / 60)}m`;
}

// Умный парсинг имени процесса
function getProcessName(path) {
  if (!path) return 'Служба ОС';
  if (path.includes('Antigravity') || path.includes('antigravity')) {
    return 'Antigravity';
  }
  const parts = path.split(/\\|\//);
  return parts[parts.length - 1];
}

function isSystemProcess(name) {
  return name === 'Служба ОС' || name.toLowerCase().includes('svchost');
}

// Обновление Select (вместо Datalist)
function updateProcessSelect() {
  currentConnections.forEach(c => {
    globalUniqueProcesses.add(getProcessName(c.metadata?.processPath));
  });
  
  processDatalist.innerHTML = '';
  
  Array.from(globalUniqueProcesses).sort().forEach(proc => {
    const option = document.createElement('option');
    option.value = proc;
    processDatalist.appendChild(option);
  });
}

// Обновление UI кнопок сортировки
function updateSortButtonsUI() {
  sortBtns.forEach(btn => {
    const type = btn.dataset.sort;
    const dir = sortState[type];
    
    // Сбрасываем текст
    if (type === 'time') btn.textContent = 'По времени';
    if (type === 'traffic') btn.textContent = 'По трафику';
    if (type === 'process') btn.textContent = 'По процессу';

    if (dir === 'desc') {
      btn.textContent += ' ⬇';
      btn.classList.add('active');
    } else if (dir === 'asc') {
      btn.textContent += ' ⬆';
      btn.classList.add('active');
    } else {
      btn.classList.remove('active');
    }
  });
}

// Рендер карточек
function renderList() {
  try {
    const pFilter = filterProcess.value.toLowerCase();
    const dFilter = filterDomain.value.toLowerCase();
    const rFilter = filterRoute.value;

    if (document.activeElement !== filterProcess) {
        updateProcessSelect();
    }

    let filtered = currentConnections.filter(c => {
      // Игнорируем DNS (порт 53)
      if (c.metadata?.destinationPort === 53) return false;
      
      const process = getProcessName(c.metadata?.processPath).toLowerCase();
      const host = (c.metadata?.host || c.metadata?.destinationIP || '').toLowerCase();
      let route = (c.chains && c.chains.length > 0 ? c.chains[0] : 'unknown').toLowerCase();
      
      // Игнорируем block
      if (route.includes('block')) return false;
      
      if (pFilter && process !== pFilter) return false; // Точное совпадение по селекту
      if (dFilter && !host.includes(dFilter)) return false;
      
      if (rFilter === 'proxy' && !route.includes('proxy') && !route.includes('vless')) return false;
      if (rFilter === 'direct' && !route.includes('direct')) return false;
      
      return true;
    });

    // Сортировка
    filtered.sort((a, b) => {
      const procA_name = getProcessName(a.metadata?.processPath);
      const procB_name = getProcessName(b.metadata?.processPath);
      const isSysA = isSystemProcess(procA_name) ? 1 : 0;
      const isSysB = isSystemProcess(procB_name) ? 1 : 0;

      // Первичная сортировка: Обычные приложения всегда выше системных
      if (isSysA !== isSysB) {
        return isSysA - isSysB; // 0 (App) перед 1 (System)
      }

      // Вторичная сортировка в зависимости от выбранного режима
      const dir = sortState[currentSort];
      if (dir === 'none') return 0; // Сохраняем группировку Apps/System
      
      const modifier = dir === 'desc' ? -1 : 1;

      if (currentSort === 'traffic') {
        const trafficA = (a.upload || 0) + (a.download || 0);
        const trafficB = (b.upload || 0) + (b.download || 0);
        return (trafficA - trafficB) * modifier;
      }
      if (currentSort === 'process') {
        return procA_name.localeCompare(procB_name) * modifier;
      }
      if (currentSort === 'time') {
        const timeA = new Date(a.start).getTime();
        const timeB = new Date(b.start).getTime();
        return (timeA - timeB) * modifier;
      }
      
      return 0;
    });

    // Обновление счетчика
    let proxyCount = 0;
    let directCount = 0;
    filtered.forEach(c => {
       const routeName = c.chains && c.chains.length > 0 ? c.chains[0] : 'direct';
       if (routeName.includes('proxy') || routeName.includes('vless')) proxyCount++;
       else directCount++;
    });
    activeCountSpan.innerHTML = `<span style="color: var(--vpn-color);">VPN: ${proxyCount}</span> <span style="color: var(--border);">|</span> <span style="color: var(--direct-color);">DIRECT: ${directCount}</span>`;

    // Группировка дублей
    const groupedMap = new Map();
    filtered.forEach(c => {
      const processName = getProcessName(c.metadata?.processPath);
      const hostStr = c.metadata?.host || c.metadata?.destinationIP || 'Неизвестно';
      const port = c.metadata?.destinationPort ? `:${c.metadata.destinationPort}` : '';
      const routeName = c.chains && c.chains.length > 0 ? c.chains[0] : 'direct';
      const rule = c.rule || '';
      const netType = c.metadata?.network ? c.metadata.network.toUpperCase() : 'TCP';
      
      const key = `${processName}|${hostStr}${port}|${routeName}|${rule}|${netType}`;
      if (groupedMap.has(key)) {
        const existing = groupedMap.get(key);
        existing.download = (existing.download || 0) + (c.download || 0);
        existing.upload = (existing.upload || 0) + (c.upload || 0);
        if (new Date(c.start).getTime() < new Date(existing.start).getTime()) {
          existing.start = c.start;
        }
      } else {
        groupedMap.set(key, { ...c });
      }
    });

    const groupedFiltered = Array.from(groupedMap.values());

    listContainer.innerHTML = '';

    groupedFiltered.forEach(c => {
      const card = document.createElement('div');
      card.className = 'conn-card';

      // 1-я строка
      const row1 = document.createElement('div');
      row1.className = 'conn-row';
      
      const domainBox = document.createElement('div');
      domainBox.className = 'conn-domain';
      let hostStr = c.metadata?.host || c.metadata?.destinationIP || 'Неизвестно';
      let port = c.metadata?.destinationPort ? `:${c.metadata.destinationPort}` : '';
      domainBox.textContent = `${hostStr}${port}`;
      
      const timeBox = document.createElement('div');
      timeBox.className = 'conn-duration';
      timeBox.textContent = c.start ? formatTime(c.start) : '-';

      row1.appendChild(domainBox);
      row1.appendChild(timeBox);

      // 2-я строка
      const row2 = document.createElement('div');
      row2.className = 'conn-row';
      
      const processBox = document.createElement('div');
      processBox.className = 'conn-process';
      const netType = c.metadata?.network ? c.metadata.network.toUpperCase() : 'TCP';
      processBox.innerHTML = `<span class="conn-network">[${netType}]</span> <span>${getProcessName(c.metadata?.processPath)}</span>`;
      if (c.metadata?.processPath) {
          processBox.title = c.metadata.processPath;
      }
      
      const trafficBox = document.createElement('div');
      trafficBox.className = 'conn-traffic';
      trafficBox.textContent = `↓ ${formatBytes(c.download || 0)}  ↑ ${formatBytes(c.upload || 0)}`;

      row2.appendChild(processBox);
      row2.appendChild(trafficBox);

      // 3-я строка
      const row3 = document.createElement('div');
      row3.className = 'conn-row';
      row3.style.marginTop = '2px';
      
      const ruleBox = document.createElement('div');
      ruleBox.className = 'conn-rule';
      ruleBox.textContent = c.rule ? `rule=${c.rule}` : '';
      ruleBox.title = c.rule || '';
      
      const routeBox = document.createElement('div');
      routeBox.className = 'conn-route';
      
      const routeName = c.chains && c.chains.length > 0 ? c.chains[0] : 'direct';
      if (routeName.includes('proxy') || routeName.includes('vless')) {
        routeBox.textContent = 'VPN';
        routeBox.classList.add('route-proxy');
      } else {
        routeBox.textContent = 'DIRECT';
        routeBox.classList.add('route-direct');
      }

      row3.appendChild(ruleBox);
      row3.appendChild(routeBox);

      card.appendChild(row1);
      card.appendChild(row2);
      card.appendChild(row3);
      
      listContainer.appendChild(card);
    });
  } catch (err) {
    console.error("Ошибка в renderList:", err);
  }
}

let pollingInterval = null;

async function fetchConnections() {
  try {
    const res = await fetch('http://127.0.0.1:9090/connections');
    if (!res.ok) return;
    const data = await res.json();
    if (data && Array.isArray(data.connections)) {
      currentConnections = data.connections;
      renderList();
    }
  } catch (err) {
    // silently ignore fetch errors to avoid spamming console
  }
}

function startPolling() {
  fetchConnections(); // fetch immediately
  if (pollingInterval) clearInterval(pollingInterval);
  pollingInterval = setInterval(fetchConnections, 1000); // 1 sec interval
}

// Слушатели фильтров
filterProcess.addEventListener('input', renderList);
filterDomain.addEventListener('input', renderList);
filterRoute.addEventListener('change', renderList);

// Слушатели сортировки (Цикл: desc -> asc -> none)
sortBtns.forEach(btn => {
  btn.addEventListener('click', (e) => {
    const type = e.target.dataset.sort;
    
    // Если кликнули на другую колонку, сбрасываем остальные в none
    if (currentSort !== type) {
      sortState[currentSort] = 'none';
      currentSort = type;
      sortState[type] = 'desc';
    } else {
      // Переключаем текущую
      if (sortState[type] === 'desc') sortState[type] = 'asc';
      else if (sortState[type] === 'asc') sortState[type] = 'none';
      else sortState[type] = 'desc';
    }
    
    updateSortButtonsUI();
    renderList();
  });
});

// Запуск
startPolling();
