// ============================================================
// connections.js — Логика окна "Монитор соединений"
// ============================================================

let ws = null;
let currentConnections = [];

const tableBody = document.getElementById('connections-body');
const filterProcess = document.getElementById('filter-process');
const filterDomain = document.getElementById('filter-domain');
const filterType = document.getElementById('filter-type');
const btnClear = document.getElementById('btn-clear-connections');

function formatBytes(bytes) {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
}

function formatTime(startTimeStr) {
  const start = new Date(startTimeStr);
  const diff = Math.floor((Date.now() - start.getTime()) / 1000); // сек
  if (diff < 60) return `${diff}s`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ${diff % 60}s`;
  return `${Math.floor(diff / 3600)}h ${Math.floor((diff % 3600) / 60)}m`;
}

function getProcessName(path) {
  if (!path) return 'Неизвестно';
  const parts = path.split(/\\|\//);
  return parts[parts.length - 1];
}

function renderTable() {
  const pFilter = filterProcess.value.toLowerCase();
  const dFilter = filterDomain.value.toLowerCase();
  const tFilter = filterType.value;

  tableBody.innerHTML = '';

  const filtered = currentConnections.filter(c => {
    const process = getProcessName(c.metadata?.processPath).toLowerCase();
    const host = (c.metadata?.host || c.metadata?.destinationIP || '').toLowerCase();
    const route = (c.chains && c.chains.length > 0 ? c.chains[0] : 'unknown').toLowerCase();

    if (pFilter && !process.includes(pFilter)) return false;
    if (dFilter && !host.includes(dFilter)) return false;
    if (tFilter !== 'all' && !route.includes(tFilter)) return false;
    
    return true;
  });

  filtered.forEach(c => {
    const tr = document.createElement('tr');
    
    // Процесс
    const tdProcess = document.createElement('td');
    tdProcess.textContent = getProcessName(c.metadata?.processPath);
    tdProcess.title = c.metadata?.processPath || '';

    // Домен / IP
    const tdDomain = document.createElement('td');
    let hostStr = c.metadata?.host || '';
    if (c.metadata?.destinationIP) {
      hostStr += hostStr ? ` (${c.metadata.destinationIP})` : c.metadata.destinationIP;
    }
    tdDomain.textContent = hostStr || 'Неизвестно';

    // Маршрут (цепочка)
    const tdRoute = document.createElement('td');
    const routeName = c.chains && c.chains.length > 0 ? c.chains[0] : 'direct';
    tdRoute.textContent = routeName.toUpperCase();
    if (routeName.includes('proxy') || routeName.includes('vless')) {
      tdRoute.className = 'route-proxy';
    } else if (routeName.includes('block')) {
      tdRoute.className = 'route-block';
    } else {
      tdRoute.className = 'route-direct';
    }

    // Правило
    const tdRule = document.createElement('td');
    tdRule.textContent = c.rule || '-';

    // Трафик
    const tdTraffic = document.createElement('td');
    tdTraffic.className = 'traffic';
    tdTraffic.textContent = `↓${formatBytes(c.download)} / ↑${formatBytes(c.upload)}`;

    // Время
    const tdTime = document.createElement('td');
    tdTime.textContent = formatTime(c.start);

    tr.appendChild(tdProcess);
    tr.appendChild(tdDomain);
    tr.appendChild(tdRoute);
    tr.appendChild(tdRule);
    tr.appendChild(tdTraffic);
    tr.appendChild(tdTime);
    
    tableBody.appendChild(tr);
  });
}

function connectWebSocket() {
  // sing-box поднимает clash_api на 9090
  ws = new WebSocket('ws://127.0.0.1:9090/connections');
  
  ws.onopen = () => {
    console.log('Подключено к sing-box API');
  };

  ws.onmessage = (event) => {
    try {
      const data = JSON.parse(event.data);
      if (data && Array.isArray(data.connections)) {
        currentConnections = data.connections;
        renderTable();
      }
    } catch (err) {
      console.error('Ошибка парсинга:', err);
    }
  };

  ws.onclose = () => {
    console.log('Отключено. Переподключение через 3 секунды...');
    currentConnections = [];
    renderTable();
    setTimeout(connectWebSocket, 3000);
  };
  
  ws.onerror = (err) => {
    console.error('WebSocket ошибка:', err);
    ws.close();
  };
}

// Слушатели фильтров
filterProcess.addEventListener('input', renderTable);
filterDomain.addEventListener('input', renderTable);
filterType.addEventListener('change', renderTable);
btnClear.addEventListener('click', () => {
  currentConnections = [];
  renderTable();
  // Вызов REST API sing-box для закрытия всех соединений (опционально)
  fetch('http://127.0.0.1:9090/connections', { method: 'DELETE' }).catch(console.error);
});

// Запуск
connectWebSocket();
