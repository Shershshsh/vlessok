// ============================================================
// connections.js — Логика окна "Монитор соединений" v2
// ============================================================

let ws = null;
let currentConnections = [];
let currentSort = 'time'; // time, traffic, process

const listContainer = document.getElementById('connections-list');
const filterProcess = document.getElementById('filter-process');
const filterDomain = document.getElementById('filter-domain');
const filterRoute = document.getElementById('filter-route');
const btnClear = document.getElementById('btn-clear');
const datalistProcesses = document.getElementById('active-processes');
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

// Обновление Datalist
function updateProcessDatalist() {
  const uniqueProcesses = new Set();
  currentConnections.forEach(c => {
    uniqueProcesses.add(getProcessName(c.metadata?.processPath));
  });
  
  datalistProcesses.innerHTML = '';
  Array.from(uniqueProcesses).sort().forEach(proc => {
    const option = document.createElement('option');
    option.value = proc;
    datalistProcesses.appendChild(option);
  });
}

// Рендер карточек
function renderList() {
  try {
    const pFilter = filterProcess.value.toLowerCase();
    const dFilter = filterDomain.value.toLowerCase();
    const rFilter = filterRoute.value;

    // Обновляем список доступных процессов (но не прерываем ввод пользователя)
    if (document.activeElement !== filterProcess) {
        updateProcessDatalist();
    }

    let filtered = currentConnections.filter(c => {
      const process = getProcessName(c.metadata?.processPath).toLowerCase();
      const host = (c.metadata?.host || c.metadata?.destinationIP || '').toLowerCase();
      let route = (c.chains && c.chains.length > 0 ? c.chains[0] : 'unknown').toLowerCase();
      
      // Игнорируем block
      if (route.includes('block')) return false;
      
      if (pFilter && !process.includes(pFilter)) return false;
      if (dFilter && !host.includes(dFilter)) return false;
      
      if (rFilter === 'proxy' && !route.includes('proxy') && !route.includes('vless')) return false;
      if (rFilter === 'direct' && !route.includes('direct')) return false;
      
      return true;
    });

    // Сортировка
    filtered.sort((a, b) => {
      if (currentSort === 'traffic') {
        const trafficA = (a.upload || 0) + (a.download || 0);
        const trafficB = (b.upload || 0) + (b.download || 0);
        return trafficB - trafficA; // по убыванию
      }
      if (currentSort === 'process') {
        const procA = getProcessName(a.metadata?.processPath).toLowerCase();
        const procB = getProcessName(b.metadata?.processPath).toLowerCase();
        return procA.localeCompare(procB); // А-Я
      }
      // по времени (по умолчанию)
      const timeA = new Date(a.start).getTime();
      const timeB = new Date(b.start).getTime();
      return timeA - timeB; // старые сверху
    });

    // Обновление счетчика
    let proxyCount = 0;
    let directCount = 0;
    filtered.forEach(c => {
       const routeName = c.chains && c.chains.length > 0 ? c.chains[0] : 'direct';
       if (routeName.includes('proxy') || routeName.includes('vless')) proxyCount++;
       else directCount++;
    });
    activeCountSpan.textContent = `VPN: ${proxyCount} | DIRECT: ${directCount}`;

    listContainer.innerHTML = '';

    filtered.forEach(c => {
      const card = document.createElement('div');
      card.className = 'conn-card';

      // 1-я строка: Домен/IP:Порт и Время
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

      // 2-я строка: Процесс+Сеть и Трафик
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

      // 3-я строка: Правило и Маршрут
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

function connectWebSocket() {
  ws = new WebSocket('ws://127.0.0.1:9090/connections');
  
  ws.onopen = () => {
    console.log('Подключено к sing-box API');
  };

  ws.onmessage = (event) => {
    try {
      const data = JSON.parse(event.data);
      if (data && Array.isArray(data.connections)) {
        currentConnections = data.connections;
        renderList();
      }
    } catch (err) {
      console.error('Ошибка парсинга:', err);
    }
  };

  ws.onclose = () => {
    console.log('Отключено. Переподключение через 3 секунды...');
    currentConnections = [];
    renderList();
    setTimeout(connectWebSocket, 3000);
  };
  
  ws.onerror = (err) => {
    console.error('WebSocket ошибка:', err);
    ws.close();
  };
}

// Слушатели фильтров
filterProcess.addEventListener('input', renderList);
filterDomain.addEventListener('input', renderList);
filterRoute.addEventListener('change', renderList);

// Слушатели сортировки
sortBtns.forEach(btn => {
  btn.addEventListener('click', (e) => {
    sortBtns.forEach(b => b.classList.remove('active'));
    e.target.classList.add('active');
    currentSort = e.target.dataset.sort;
    renderList();
  });
});

// Кнопка очистки
btnClear.addEventListener('click', () => {
  if(!confirm('Принудительно закрыть все текущие соединения? Это может прервать активные загрузки.')) return;
  currentConnections = [];
  renderList();
  fetch('http://127.0.0.1:9090/connections', { method: 'DELETE' }).catch(console.error);
});

// Запуск
connectWebSocket();
