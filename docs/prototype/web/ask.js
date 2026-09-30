// HDMI confirmation dialogs in the launcher style. One component for every
// ConfirmationKind: a panel anchored to the row that asked (like the combo
// drop-downs), or centred over a dimmed page when nothing on screen asked.
// The accent follows the page: violet in Settings, red in Arcade.
window.ASK = (() => {
  const CONTENT_TOP = 77, CONTENT_BOTTOM = 499, EDGE = 934;
  let el, scrim, foot, cur = null;

  // Copy per ConfirmationKind (launcher_bridge.rs), tightened for the panel.
  const KINDS = {
    'exit-to-mister': { title: 'Exit to MiSTer?', msg: 'Use the stock MiSTer menu until the next reboot.', left: 'CANCEL', right: 'EXIT' },
    'refresh-database': { title: 'Refresh database?', msg: 'Rescan changed systems in the background. Games and screenshots stay available.', left: 'CANCEL', right: 'REFRESH' },
    'database-refresh-unavailable': { title: 'Refresh unavailable', msg: 'A library update is already running. Try again when it finishes.', left: 'OK' },
    'restart': { title: 'Restart MiSTer?', msg: 'Reboot the MiSTer now.', left: 'CANCEL', right: 'RESTART' },
    'library-changed': { title: 'Library changed', msg: 'New games were found. Keep the current library or rebuild it now.', left: 'CONTINUE', right: 'REBUILD' },
    'library-update-failed': { title: 'Library update failed', msg: 'Continuing with the current library. Try rebuilding again later.', left: 'OK' },
    'display-resolution-error': { title: 'Resolution not changed', msg: 'The display resolution could not be changed.', left: 'OK' },
    'add-favourite': { msg: 'Add this game to Favourites?', left: 'CANCEL', right: 'ADD' },
    'remove-favourite': { msg: 'Remove this game from Favourites?', left: 'CANCEL', right: 'REMOVE' },
  };

  function init(stage) {
    scrim = document.createElement('div'); scrim.id = 'askScrim';
    el = document.createElement('div'); el.id = 'ask';
    foot = document.createElement('div'); foot.id = 'askFoot';
    stage.append(scrim, el, foot);
  }

  // spec: { kind | title,msg,left,right, value, countdown, accent: 'violet'|'red',
  //         anchor: {top, h, left, right} | null, onChoose(choice) }
  function open(spec) {
    cur = { focus: 0, ...KINDS[spec.kind], ...spec };
    const red = cur.accent === 'red';
    el.className = red ? 'red' : '';
    scrim.className = cur.anchor ? '' : 'on';
    document.getElementById('stage').classList.add('ask-open', cur.anchor ? (red ? 'ask-arc' : 'ask-set') : 'ask-modal');
    render();
    el.offsetHeight; el.classList.add('show');      // reflow first so it still animates in
    if (cur.countdown) cur.timer = setInterval(() => { if (--cur.countdown <= 0) choose(0); else render(); }, 1000);
  }
  function render() {
    const c = cur, btns = [c.left, c.right].filter(Boolean);
    const label = (b, i) => i === 0 && c.countdown ? `${b} ${c.countdown}` : b;
    el.innerHTML = `
      ${c.title ? `<div class="t h">${c.title}</div>` : ''}
      ${c.value ? `<div class="v m">${c.value}</div>` : ''}
      <div class="q m">${c.msg}</div>
      <div class="btns">${btns.map((b, i) => `<div class="btn${i === c.focus ? ' hi' : ''}"><span class="m">${label(b, i)}</span></div>`).join('')}</div>`;
    foot.innerHTML = `<div class="a m foot" style="left:30px">A &nbsp;${btns.length > 1 ? 'CHOOSE' : 'OK'}</div>
      ${btns.length > 1 ? `<div class="a m foot" style="left:154px">B &nbsp;${c.left}</div><div class="a m foot" style="left:612px">LEFT RIGHT &nbsp;MOVE</div>` : ''}`;
    place();
  }
  function place() {
    const a = cur.anchor, w = a ? a.w || 400 : 440;
    el.style.width = w + 'px';
    const h = el.offsetHeight;
    if (!a) {
      el.classList.remove('up');
      el.style.left = (480 - w / 2) + 'px';
      el.style.top = Math.round((CONTENT_TOP + CONTENT_BOTTOM - h) / 2) + 'px';
      return;
    }
    const below = a.top + a.h + 2, up = below + h > CONTENT_BOTTOM - 4;
    el.classList.toggle('up', up);
    el.style.left = (a.left != null ? a.left : EDGE - w) + 'px';
    el.style.top = (up ? a.top - 2 - h : below) + 'px';
  }
  function close() {
    if (!cur) return;
    clearInterval(cur.timer);
    el.classList.remove('show'); scrim.className = '';
    document.getElementById('stage').classList.remove('ask-open', 'ask-arc', 'ask-set', 'ask-modal');
    const done = cur.onClose; cur = null; done && done();
  }
  function choose(i) { const c = cur; close(); c.onChoose && c.onChoose(i); }
  function key(k) {
    if (!cur) return false;
    const n = [cur.left, cur.right].filter(Boolean).length;
    if (k === 'ArrowLeft' || k === 'ArrowRight') { cur.focus = n > 1 ? 1 - cur.focus : 0; render(); }
    else if (k === 'Enter' || k === 'a') choose(cur.focus);
    else if (k === 'Escape' || k === 'b' || k === 'Backspace') choose(0);
    return true;
  }
  return { init, open, close, key, KINDS, get active() { return !!cur; } };
})();
