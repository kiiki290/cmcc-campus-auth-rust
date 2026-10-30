// 校园宽带自动认证 — 前端状态机（原生 JS，无框架）
//
// 与后端的契约：
//   attach_events(channel) -> Bootstrap（含最近日志，日志带单调 id）
//   之后后端通过 Channel 推送 { logs, snapshot? }
//   snapshot 字段：phase / stage / busy / monitoring / wanIp / lastCheck / browser / fatal

(() => {
  'use strict';

  const MAX_LOGS = 1500; // JS 内存上限
  const MAX_LOG_NODES = 600; // DOM 节点上限
  const STAGE_ORDER = [
    'probe_done',
    'portal_loaded',
    'captcha_ready',
    'submitted',
    'releasing',
  ];
  const PHASE_LABEL = {
    idle: '待机',
    probing: '检测中',
    online: '在线',
    offline: '断线',
    authenticating: '认证中',
    error: '异常',
  };

  const $ = (id) => document.getElementById(id);
  const els = {
    version: $('version'),
    phaseChip: $('phase-chip'),
    monitorChip: $('monitor-chip'),
    busySpinner: $('busy-spinner'),
    settingsButton: $('btn-settings'),

    viewControl: $('view-control'),
    viewSettings: $('view-settings'),

    statusCard: $('status-card'),
    phaseTitle: $('phase-title'),
    phaseSub: $('phase-sub'),
    stageBar: $('stage-bar'),
    factAccount: $('fact-account'),
    factPassword: $('fact-password'),
    factInterval: $('fact-interval'),
    factIp: $('fact-ip'),
    factLast: $('fact-last'),

    notice: $('notice'),
    noticeText: $('notice-text'),
    noticeClose: $('notice-close'),

    btnAuth: $('btn-auth'),
    btnStart: $('btn-start'),
    btnStop: $('btn-stop'),
    btnCancel: $('btn-cancel'),
    busyRow: $('busy-row'),
    firstUse: $('first-use'),
    gotoSettings: $('btn-goto-settings'),

    logCount: $('log-count'),
    logList: $('log-list'),
    autoScroll: $('auto-scroll'),
    logClear: $('log-clear'),

    back: $('btn-back'),
    dirtyHint: $('dirty-hint'),
    form: $('settings-form'),
    studentId: $('student-id'),
    password: $('password'),
    passwordState: $('password-state'),
    interval: $('interval'),
    browserMode: $('browser-mode'),
    customPathRow: $('custom-path-row'),
    customPath: $('custom-path'),
    browse: $('btn-browse'),
    testBrowser: $('btn-test-browser'),
    browserResult: $('browser-result'),
    autoStartMonitor: $('auto-start-monitor'),
    closeToTray: $('close-to-tray'),
    autostart: $('autostart'),
    trayHint: $('tray-hint'),
    save: $('btn-save'),
    clear: $('btn-clear'),

    footerBrowser: $('footer-browser'),
    footerConfig: $('footer-config'),

    fatalDialog: $('fatal-dialog'),
    fatalText: $('fatal-text'),
    fatalOk: $('fatal-ok'),

    clearDialog: $('clear-dialog'),
    clearCancel: $('clear-cancel'),
    clearConfirm: $('clear-confirm'),
  };

  const tauri = window.__TAURI__;
  const invoke = tauri
    ? tauri.core.invoke
    : async () => {
        throw '未检测到 Tauri 运行环境';
      };

  let boot = null;
  let snapshot = null;
  let logs = [];
  let lastLogId = 0;
  let ready = false;
  let pendingBatches = [];
  let currentView = 'control';
  let invoking = false;
  let fatalDismissed = null;
  let noticeTimer = 0;

  // ── 视图与提示 ──────────────────────────────────────────

  function selectView(name) {
    currentView = name;
    const settings = name === 'settings';
    els.viewControl.hidden = settings;
    els.viewSettings.hidden = !settings;
    els.settingsButton.title = settings ? '返回控制' : '设置';
    els.settingsButton.setAttribute('aria-label', settings ? '返回控制' : '设置');
    renderDirty();
  }

  function setNotice(text, tone = 'warn', autoMs = 0) {
    window.clearTimeout(noticeTimer);
    els.noticeText.textContent = text;
    els.notice.className = tone === 'info' ? 'notice info' : tone === 'error' ? 'notice error' : 'notice';
    els.notice.hidden = false;
    if (autoMs > 0) {
      noticeTimer = window.setTimeout(clearNotice, autoMs);
    }
  }

  function clearNotice() {
    window.clearTimeout(noticeTimer);
    els.notice.hidden = true;
  }

  // ── 凭据与草稿 ──────────────────────────────────────────

  function hasCredentials() {
    return !!(
      boot &&
      boot.config.student_id.trim() !== '' &&
      boot.hasPassword
    );
  }

  function clampInterval(value) {
    const parsed = Number.parseInt(value, 10);
    if (Number.isNaN(parsed)) return boot ? boot.config.poll_interval : 5;
    return Math.min(3600, Math.max(3, parsed));
  }

  function readDraft() {
    const password = els.password.value;
    return {
      studentId: els.studentId.value,
      password: password === '' ? null : password,
      pollInterval: clampInterval(els.interval.value),
      browserMode: els.browserMode.value,
      customBrowserPath: els.customPath.value,
      closeToTray: els.closeToTray.checked,
      autoStartMonitor: els.autoStartMonitor.checked,
      autostart: els.autostart.checked,
    };
  }

  function isDirty() {
    if (!boot) return false;
    const cfg = boot.config;
    return (
      els.studentId.value.trim() !== cfg.student_id ||
      els.password.value !== '' ||
      clampInterval(els.interval.value) !== cfg.poll_interval ||
      els.browserMode.value !== cfg.browser_mode ||
      els.customPath.value.trim() !== cfg.custom_browser_path ||
      els.autoStartMonitor.checked !== cfg.auto_start_monitor ||
      els.closeToTray.checked !== cfg.close_to_tray ||
      els.autostart.checked !== cfg.autostart
    );
  }

  function renderDirty() {
    els.dirtyHint.hidden = !isDirty();
  }

  // ── 数据应用 ────────────────────────────────────────────

  function applyBootstrap(data, { resetLog = true } = {}) {
    boot = data;
    hydrateSettings(data);
    renderStatic();
    if (resetLog) resetLogs(data.logs);
    applySnapshot(data.snapshot);
  }

  function renderStatic() {
    if (!boot) return;
    const cfg = boot.config;

    els.version.textContent = `v${boot.version}`;
    els.footerConfig.textContent = `配置：${boot.configPath}`;
    els.footerConfig.title = boot.configPath;

    if (cfg.student_id.trim() === '') {
      els.factAccount.textContent = '未设置';
      els.factAccount.className = 'fact-missing';
    } else {
      els.factAccount.textContent = boot.maskedStudentId;
      els.factAccount.className = '';
    }

    if (boot.hasPassword) {
      els.factPassword.textContent = '已保存到系统密钥环';
      els.factPassword.className = 'fact-ok';
    } else {
      els.factPassword.textContent = '未保存';
      els.factPassword.className = 'fact-missing';
    }

    els.factInterval.textContent = `${cfg.poll_interval} 秒`;
    els.firstUse.hidden = hasCredentials();

    if (!boot.trayOk) {
      els.closeToTray.disabled = true;
      els.trayHint.hidden = false;
      els.trayHint.textContent =
        '系统托盘不可用，关闭窗口将直接退出（GNOME 请启用 AppIndicator/Ubuntu AppIndicators 扩展）。';
    } else {
      els.closeToTray.disabled = false;
      els.trayHint.hidden = true;
      els.trayHint.textContent = '';
    }
  }

  function hydrateSettings(data) {
    const cfg = data.config;
    els.studentId.value = cfg.student_id;
    els.password.value = '';
    els.password.placeholder = data.hasPassword ? '已保存，留空不修改' : '请输入密码';
    els.passwordState.textContent = data.hasPassword
      ? '密码已保存到系统密钥环'
      : '尚未保存密码';
    els.passwordState.className = data.hasPassword ? 'hint fact-ok' : 'hint fact-missing';
    els.interval.value = String(cfg.poll_interval);
    els.browserMode.value = cfg.browser_mode;
    els.customPath.value = cfg.custom_browser_path;
    els.autoStartMonitor.checked = cfg.auto_start_monitor;
    els.closeToTray.checked = cfg.close_to_tray;
    els.autostart.checked = cfg.autostart;
    updateCustomRow();
    renderDirty();
  }

  function applySnapshot(state) {
    if (!state) return;
    snapshot = state;

    const label = PHASE_LABEL[state.phase] || state.phase;
    els.statusCard.dataset.phase = state.phase;
    els.phaseTitle.textContent = label;
    els.phaseChip.textContent = label;
    els.phaseChip.dataset.phase = state.phase;
    els.phaseSub.textContent = subtitleFor(state);

    els.monitorChip.hidden = !state.monitoring;
    els.busySpinner.hidden = !(state.busy && !state.monitoring);
    els.busyRow.hidden = !state.busy;
    els.btnCancel.hidden = !(state.busy && !state.monitoring);

    const canAuth = hasCredentials() && !state.busy && !state.monitoring;
    els.btnAuth.disabled = !canAuth;
    els.btnStart.disabled = !canAuth;
    els.btnStop.disabled = !state.monitoring;

    els.factIp.textContent = state.wanIp || '--';
    els.factLast.textContent = state.lastCheck || '--';
    els.footerBrowser.textContent = state.browser || '未检测';

    renderStage(state);
    renderFatal(state);
  }

  function subtitleFor(state) {
    if (state.monitoring) {
      return state.busy ? '监控运行中 · 正在认证' : '监控运行中 · 断线自动重连';
    }
    if (state.busy) return '正在执行认证任务';
    return '监控已停止';
  }

  function renderStage(state) {
    let index = -1;
    if (state.phase === 'online') {
      index = STAGE_ORDER.length - 1;
    } else if (state.stage) {
      index = STAGE_ORDER.indexOf(state.stage);
    }
    const segments = els.stageBar.children;
    for (let i = 0; i < segments.length; i += 1) {
      const segment = segments[i];
      segment.classList.toggle('on', i <= index);
      segment.classList.toggle('done', state.phase === 'online');
      segment.classList.toggle(
        'err',
        state.phase === 'error' && i === index,
      );
    }
  }

  function renderFatal(state) {
    if (!state.fatal) {
      fatalDismissed = null;
      if (els.fatalDialog.open) els.fatalDialog.close();
      return;
    }
    if (state.fatal === fatalDismissed) return;
    els.fatalText.textContent = state.fatal;
    if (!els.fatalDialog.open) els.fatalDialog.showModal();
  }

  function closeFatal() {
    fatalDismissed = snapshot ? snapshot.fatal : null;
    if (els.fatalDialog.open) els.fatalDialog.close();
    invoke('dismiss_fatal').catch(() => {});
  }

  // ── 日志 ────────────────────────────────────────────────

  function logNode(record) {
    const line = document.createElement('div');
    line.className = `log-line level-${record.level}`;
    const time = document.createElement('span');
    time.className = 'log-time';
    time.textContent = `[${record.time}] `;
    const text = document.createElement('span');
    text.className = 'log-text';
    text.textContent = record.text;
    line.append(time, text);
    return line;
  }

  function appendLogs(records) {
    if (!records || records.length === 0) return;
    let added = false;
    const fragment = document.createDocumentFragment();
    for (const record of records) {
      if (record.id <= lastLogId) continue;
      lastLogId = record.id;
      logs.push(record);
      if (logs.length > MAX_LOGS) logs.shift();
      fragment.appendChild(logNode(record));
      added = true;
    }
    if (!added) return;

    els.logList.appendChild(fragment);
    while (els.logList.childElementCount > MAX_LOG_NODES) {
      els.logList.firstElementChild.remove();
    }
    els.logCount.textContent = `${logs.length} 条`;
    if (els.autoScroll.checked) {
      window.requestAnimationFrame(() => {
        els.logList.scrollTop = els.logList.scrollHeight;
      });
    }
  }

  function resetLogs(records) {
    logs = [];
    lastLogId = 0;
    els.logList.replaceChildren();
    els.logCount.textContent = '0 条';
    appendLogs(records || []);
  }

  function clearLogs() {
    logs = [];
    els.logList.replaceChildren();
    els.logCount.textContent = '0 条';
  }

  // ── 动作 ────────────────────────────────────────────────

  async function saveSettings() {
    const data = await invoke('save_settings', { input: readDraft() });
    applyBootstrap(data, { resetLog: false });
    els.password.value = '';
    els.password.placeholder = data.hasPassword ? '已保存，留空不修改' : '请输入密码';
    renderDirty();
  }

  async function runAction(command) {
    if (invoking) return;
    invoking = true;
    try {
      if (isDirty()) {
        await saveSettings();
      }
      if (!hasCredentials()) {
        selectView('settings');
        setNotice('请先填写并保存学号与密码');
        return;
      }
      await invoke(command);
      clearNotice();
    } catch (error) {
      setNotice(String(error), 'error');
    } finally {
      invoking = false;
    }
  }

  async function runQuiet(command) {
    try {
      await invoke(command);
    } catch (error) {
      setNotice(String(error), 'error');
    }
  }

  function updateCustomRow() {
    els.customPathRow.hidden = els.browserMode.value !== 'custom';
  }

  // ── 事件绑定 ────────────────────────────────────────────

  function bind() {
    els.settingsButton.addEventListener('click', () => {
      selectView(currentView === 'control' ? 'settings' : 'control');
    });
    els.back.addEventListener('click', () => selectView('control'));
    els.gotoSettings.addEventListener('click', () => selectView('settings'));

    els.btnAuth.addEventListener('click', () => runAction('auth_now'));
    els.btnStart.addEventListener('click', () => runAction('start_monitor'));
    els.btnStop.addEventListener('click', () => runQuiet('stop_monitor'));
    els.btnCancel.addEventListener('click', () => runQuiet('cancel_auth'));

    els.noticeClose.addEventListener('click', clearNotice);
    els.logClear.addEventListener('click', clearLogs);

    els.form.addEventListener('submit', async (event) => {
      event.preventDefault();
      try {
        await saveSettings();
        setNotice('设置已保存', 'info', 2500);
      } catch (error) {
        setNotice(String(error), 'error');
      }
    });

    for (const input of [
      els.studentId,
      els.password,
      els.interval,
      els.customPath,
      els.autoStartMonitor,
      els.closeToTray,
      els.autostart,
    ]) {
      input.addEventListener('input', renderDirty);
      input.addEventListener('change', renderDirty);
    }

    els.browserMode.addEventListener('change', () => {
      updateCustomRow();
      renderDirty();
    });

    els.interval.addEventListener('blur', () => {
      const value = clampInterval(els.interval.value);
      els.interval.value = String(value);
      renderDirty();
    });

    els.browse.addEventListener('click', async () => {
      try {
        const path = await invoke('pick_browser_path');
        if (path) {
          els.customPath.value = path;
          els.browserMode.value = 'custom';
          updateCustomRow();
          renderDirty();
        }
      } catch (error) {
        setNotice(String(error), 'error');
      }
    });

    els.testBrowser.addEventListener('click', async () => {
      els.browserResult.textContent = '检测中…';
      try {
        const description = await invoke('test_browser', { input: readDraft() });
        els.browserResult.textContent = `可用：${description}`;
        els.browserResult.title = description;
        els.footerBrowser.textContent = description;
      } catch (error) {
        els.browserResult.textContent = '不可用';
        els.browserResult.title = String(error);
        els.footerBrowser.textContent = '未检测';
        setNotice(String(error), 'error');
      }
    });

    els.clear.addEventListener('click', () => {
      if (!els.clearDialog.open) els.clearDialog.showModal();
    });
    els.clearCancel.addEventListener('click', () => els.clearDialog.close());
    els.clearConfirm.addEventListener('click', async () => {
      els.clearDialog.close();
      try {
        const data = await invoke('clear_credentials');
        applyBootstrap(data);
        setNotice('已清除已保存的账号密码', 'info', 2500);
      } catch (error) {
        setNotice(String(error), 'error');
      }
    });

    els.fatalOk.addEventListener('click', closeFatal);
    els.fatalDialog.addEventListener('cancel', (event) => {
      event.preventDefault();
      closeFatal();
    });
  }

  // ── 启动 ────────────────────────────────────────────────

  async function init() {
    bind();

    if (!window.__TAURI__) {
      setNotice('请通过桌面程序启动', 'error');
      return;
    }

    try {
      const channel = new tauri.core.Channel();
      channel.onmessage = (payload) => {
        if (!ready) {
          pendingBatches.push(payload);
          return;
        }
        applyBatch(payload);
      };

      const data = await invoke('attach_events', { channel });
      applyBootstrap(data);
      ready = true;
      for (const payload of pendingBatches) applyBatch(payload);
      pendingBatches = [];

      const hint = await invoke('ui_ready');
      if (hint && hint.needsSetup) {
        selectView('settings');
        setNotice('请先在“设置”中填写学号与密码');
      }
    } catch (error) {
      setNotice(String(error), 'error');
    }
  }

  function applyBatch(payload) {
    if (!payload) return;
    if (payload.snapshot) applySnapshot(payload.snapshot);
    if (payload.logs && payload.logs.length) appendLogs(payload.logs);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init, { once: true });
  } else {
    init();
  }
})();
