// HDMI Arcade page: a front-on cabinet with a square screen frames the
// selected game's screenshot (landscape or portrait both fit), beside a
// Settings-style list that scrolls under a fixed red focus bar. The only
// motion at rest is the list moving and the screenshots crossfading.
window.ARCADE = (() => {
  const RED = '#e7695a';
  const GAMES = [
    '1942', '1943: The Battle of Midway (Euro)', '19XX: The War Against Destiny (Europe 960104)', '280-ZZZAP',
    '4D Warriors', '720 Degrees (rev 4)', 'A.B. Cop (World)', 'Act-Fancer Cybernetick Hyper Weapon (World rev 3)',
    'Action Fighter (World, S16A) [FD1089A 317-0018]', 'Adventure Canoe', 'Adventure Quiz Capcom World 2 (Japan 920611)',
    'After Burner (Ver 1.32)', 'After Burner II', 'Ah Eikou no Koshien (Japan)', 'Air Assault (World)', 'Alien Syndrome',
    'Altered Beast (set 8)', 'Arkanoid (World)', 'Asteroids (rev 4)', 'Bubble Bobble', 'Centipede (revision 4)',
    'Donkey Kong (US set 1)', 'Double Dragon (World)', 'Elevator Action', 'Frogger', 'Galaga (Namco rev. B)',
    'Gauntlet (rev 14)', 'Golden Axe (set 6, US)', 'Ms. Pac-Man', 'Out Run (sitdown/upright, Rev B)', 'Pac-Man (Midway)',
    'Rastan (World Rev 1)', 'Robotron: 2084 (Solid Blue label)', 'Space Harrier', 'Street Fighter II: The World Warrior (World 910522)',
    'Tetris (set 1)', 'Xevious (Namco)'];
  // Portrait (vertical) games show the portrait placeholder screenshot.
  const PORTRAIT = /^(1942|1943|19XX|Action Fighter|Centipede|Donkey Kong|Frogger|Galaga|Ms\. Pac|Pac-Man|Xevious|Arkanoid)/;
  const shotFor = g => PORTRAIT.test(g) ? 'shot-actionfighter.png' : 'shot-720.png';
  const split = g => { const i = g.search(/ [(\[]/); return i < 0 ? [g, ''] : [g.slice(0, i), g.slice(i + 1)]; };

  // Geometry (960x540): list on the left, cabinet on the right.
  const LIST_X = 26, LIST_W = 462, VIEW_TOP = 124, VIEW_BOTTOM = 494, ROW = 36, FOCUS_ROW = 3;
  // Rendered cabinet (Blender MAGIK_01_ARCADE_CABINET UI camera, 483x519 at
  // 302 px/m) shown 1:1 in the 960x540 frame; the header band crops the top.
  // Its full-width screen is exactly 320x320 at (82, 61): screenshots are
  // shown unscaled inside it (320x240 letterboxed, 224x320 pillarboxed).
  const CAB = { x: 490, y: 35, w: 483, h: 519 };
  const SCREEN = { x: CAB.x + 82, y: CAB.y + 61, s: 320 };
  // The Arcade card shares that camera pose: its face is exactly this region
  // of the cabinet render, so the zoom is continuous.
  const CARD_IN_CAB = { x: 30.1, y: 28.4, w: 422.8, h: 591.9 };
  const CARD = { x: 520, y: 158, w: 180, h: 252 };      // Arcade card face on the launcher

  // Games, Recent and Favourites, like every other system's page. The list shows
  // one of them; Select swaps the left side between the list and the hub.
  const RECENT = ['720 Degrees (rev 4)', '1942', 'Ms. Pac-Man', 'Out Run (sitdown/upright, Rev B)', 'Street Fighter II: The World Warrior (World 910522)'];
  const FAVS = GAMES.filter(g => /^(1942|720|Action Fighter)/.test(g));
  let root, list, shots, sel = 5, frontShot = 0, LISTG = GAMES, amode = 'list', atile = 0, hub = null;
  function fillList() {
    list.innerHTML = LISTG.map((g, i) => {
      const [base, rest] = split(g);
      return `<div class="arow" style="position:absolute;left:0;top:${i * ROW}px;width:100%;height:${ROW}px">
        <span class="h" style="position:absolute;left:14px;top:11px;max-width:${LIST_W - 28}px;overflow:hidden">${base.toUpperCase()}
        <span class="m" style="color:var(--muted);margin-left:8px">${rest.toUpperCase()}</span></span>
        <i style="position:absolute;left:14px;right:0;bottom:0;height:1px;background:var(--rule)"></i></div>`;
    }).join('');
  }
  function buildHub() {
    const holder = document.createElement('div'); holder.id = 'arcHub';
    hub = HUB.build({ title: 'ARCADE', subtitle: 'MANY MAKERS &nbsp;/&nbsp; 1971 - 2005 &nbsp;/&nbsp; COIN-OP', accent: RED, count: '999 GAMES READY TO PLAY',
      tiles: [{ n: 999, label: 'GAMES' }, { n: RECENT.length, label: 'RECENT' }, { n: FAVS.length, label: 'FAVOURITES' }],
      captions: ['BROWSE ALL 999 ARCADE GAMES', 'PICK UP WHERE YOU LEFT OFF', `${FAVS.length} SAVED FAVOURITES`] });
    holder.append(hub.el); root.append(holder); hub.setFocus(0, false);
    applyAMode();
  }
  function setSection(i) {
    LISTG = i === 1 ? RECENT : i === 2 ? FAVS : GAMES;
    sel = i === 0 ? 5 : Math.min(FOCUS_ROW, LISTG.length - 1);
    root.querySelector('#arcLabel').textContent = 'ARCADE' + (i === 1 ? ' / RECENT' : i === 2 ? ' / FAVOURITES' : '');
    root.querySelector('#arcCount').textContent = i === 0 ? '999 GAMES' : `${LISTG.length} TITLES`;
    fillList(); render(true);
  }
  const hubParts = () => hub ? hub.bands : [];
  function applyAMode(force) {
    const showList = amode === 'list';
    root.querySelectorAll('#arcLabel,#arcCount,#arcView').forEach(el => { el.style.visibility = showList ? '' : 'hidden'; });
    hubParts().forEach(el => { el.style.visibility = showList ? 'hidden' : ''; });
    const fa = document.getElementById('footArc'), fh = document.getElementById('footArcHub');
    if (fa && fh && (force || !busy)) { fa.style.opacity = showList ? 1 : 0; fh.style.opacity = showList ? 0 : 1; }
  }
  function toggleMode() {
    if (busy || searching) return;
    const toList = amode === 'hub';
    if (toList) render(true);
    pushBands(toList ? hubParts() : listParts(), toList ? listParts() : hubParts(), toList ? 1 : -1, () => { amode = toList ? 'list' : 'hub'; applyAMode(true); });
  }
  function openSection(i) {
    if (![999, RECENT.length, FAVS.length][i]) return;
    setSection(i); toggleMode();
  }

  function build(stage) {
    root = document.createElement('div');
    root.id = 'arcade'; root.className = 'page';
    root.innerHTML = `
      <div class="a m muted" id="arcLabel" style="left:26px;top:101px">ARCADE</div>
      <div class="a m muted" id="arcCount" style="left:${LIST_X + LIST_W - 120}px;top:101px;width:120px;text-align:right">999 GAMES</div>
      <div id="arcView" style="position:absolute;left:${LIST_X}px;top:${VIEW_TOP}px;width:${LIST_W}px;height:${VIEW_BOTTOM - VIEW_TOP}px;overflow:hidden">
        <div id="arcFocus" style="position:absolute;left:0;top:${FOCUS_ROW * ROW}px;width:100%;height:${ROW}px;
          background:linear-gradient(90deg,#3a1511 0%,#1d0a08 65%,rgba(0,0,0,0) 100%)">
          <i style="position:absolute;left:0;top:0;bottom:0;width:4px;background:${RED};box-shadow:0 0 8px #a4382c"></i></div>
        <div id="arcList" style="position:absolute;left:0;top:0;width:100%;transition:transform .14s cubic-bezier(.2,.8,.2,1)"></div>
      </div>
      <div id="arcCab" style="position:absolute;left:${CAB.x}px;top:${CAB.y}px;width:${CAB.w}px;height:${CAB.h}px;transform-origin:0 0">
        <img src="arcade-cabinet.png" alt="" style="position:absolute;left:0;top:0;width:${CAB.w}px;height:${CAB.h}px">
        <i style="position:absolute;left:0;top:${CAB.h}px;width:${CAB.w}px;height:140px;background:#000"></i>
        <div id="arcScreen" style="position:absolute;left:${SCREEN.x - CAB.x}px;top:${SCREEN.y - CAB.y}px;width:${SCREEN.s}px;height:${SCREEN.s}px;overflow:hidden;background:#000">
          <img class="shot" style="position:absolute;inset:0;width:100%;height:100%;object-fit:none;image-rendering:pixelated;transition:opacity .13s linear">
          <img class="shot" style="position:absolute;inset:0;width:100%;height:100%;object-fit:none;image-rendering:pixelated;transition:opacity .13s linear;opacity:0">
        </div>
      </div>`;
    stage.insertBefore(root, document.getElementById('frame'));
    list = root.querySelector('#arcList');
    fillList();
    buildHub();
    shots = [...root.querySelectorAll('.shot')];
    render(true);
  }

  // The list moves so the selection sits on the fixed focus bar.
  function render(instant) {
    if (instant) list.style.transition = 'none';
    list.style.transform = `translateY(${(FOCUS_ROW - sel) * ROW}px)`;
    [...list.children].forEach((row, i) => {
      row.querySelector('.h').style.color = i === sel ? '#f3b5ab' : '';
      row.classList.toggle('cur', i === sel);
      row.style.opacity = Math.abs(i - sel) > 7 ? 0 : 1;
    });
    if (instant) { list.offsetHeight; list.style.transition = ''; }
    showShot(LISTG[sel]);
  }
  // Crossfade the cabinet screen to a game's screenshot.
  function showShot(game) {
    const src = game ? shotFor(game) : '';
    if (shots[frontShot].getAttribute('src') === src && shots[frontShot].style.opacity !== '0') return;
    const next = shots[1 - frontShot];
    if (src) next.src = src; else next.removeAttribute('src');
    next.style.opacity = src ? 1 : 0; shots[frontShot].style.opacity = 0; frontShot = 1 - frontShot;
  }
  function move(d) { const n = Math.max(0, Math.min(LISTG.length - 1, sel + d)); if (n !== sel) { sel = n; render(false); } }

  // ---------- card zoom: Arcade card -> Arcade page ----------
  const clamp01 = v => v < 0 ? 0 : v > 1 ? 1 : v;
  const inOut = t => t < .5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
  const out = t => 1 - Math.pow(1 - t, 4);
  const win = (t, at, dur) => clamp01((t - at) / dur);
  const TOTAL = 1000, ZMAX = 8;
  let face, outline;
  function zoomFrame(t, E) {
    const z = Math.exp(Math.log(ZMAX) * inOut(win(t, 0, 760)));
    const cx = CARD.x + CARD.w / 2, cy = CARD.y + CARD.h / 2;
    // outline: the card's red border zooming past the edges
    outline.style.transform = `scale(${z})`; outline.style.opacity = Math.max(0, 1 - Math.pow(win(t, 0, 760), 2) * 1.25);
    // the launcher's own card face scales with the window and fades out
    face.style.transform = `scale(${z})`; face.style.opacity = 1 - win(t, 60, 200);
    // the cabinet rises out of the card into place
    const pc = inOut(win(t, 80, 760));
    const s0 = CARD.w / CARD_IN_CAB.w, x0 = CARD.x - CARD_IN_CAB.x * s0, y0 = CARD.y - CARD_IN_CAB.y * s0;
    const cab = root.querySelector('#arcCab');
    const tx = (x0 - CAB.x) * (1 - pc), ty = (y0 - CAB.y) * (1 - pc), sc = s0 + (1 - s0) * pc;
    cab.style.transform = `translate(${tx}px,${ty}px) scale(${sc})`;
    // Clip the cabinet to the zooming card window: at t=0 it is exactly the
    // card face, and the rest of the cabinet appears as the window opens.
    const ww = CARD.w * z, wh = CARD.h * z, wx = cx - ww / 2, wy = cy - wh / 2;
    const L = (wx - CAB.x - tx) / sc, T = (wy - CAB.y - ty) / sc;
    const Rr = CAB.w - (wx + ww - CAB.x - tx) / sc, B = CAB.h - (wy + wh - CAB.y - ty) / sc;
    cab.style.clipPath = z >= ZMAX ? 'none' : `inset(${T}px ${Rr}px ${B}px ${L}px round ${8 * z / sc}px)`;
    cab.style.opacity = 1;
    E.launcher.style.opacity = 1 - win(t, 120, 360);
    // list label and rows deal in, then the first screenshot lights up
    if (amode === 'hub') hubParts().forEach((r, i) => { const k = out(win(t, 500 + Math.min(i, 10) * 26, 280)); r.style.opacity = k; r.style.translate = `${Math.round(40 * (1 - k))}px 0`; });
    const rows = amode === 'hub' ? [] : [root.querySelector('#arcLabel'), root.querySelector('#arcCount'), root.querySelector('#arcFocus'), ...list.children];
    rows.forEach((r, i) => {
      // stagger by on-screen position: header items first, then visible rows top to bottom
      const order = i < 3 ? i : Math.max(0, i - 3 - (sel - FOCUS_ROW)) + 1;
      const k = out(win(t, 500 + Math.min(order, 10) * 22, 280));   // all done by 1000 ms
      r.style.opacity = r.id ? k : (Math.abs(i - 3 - sel) > 7 ? 0 : k);
      if (!r.id) r.style.translate = `${Math.round(40 * (1 - k))}px 0`;
    });
    root.querySelector('#arcScreen').style.opacity = win(t, 760, 200);
    E.footL.style.opacity = 1 - win(t, 80, 160);
    const fk = win(t, 640, 220);
    document.getElementById('footArc').style.opacity = amode === 'list' ? fk : 0; document.getElementById('footArcHub').style.opacity = amode === 'hub' ? fk : 0;
  }
  let busy = false;
  function run(dir, E, slow, done) {
    if (dir > 0) { amode = 'hub'; atile = 0; hub.setFocus(0, false); setSection(0); applyAMode(true); }
    busy = true; root.classList.add('on');
    let t = dir > 0 ? 0 : TOTAL, last = performance.now();
    const step = now => {
      t = Math.max(0, Math.min(TOTAL, t + dir * (now - last) / slow)); last = now;
      zoomFrame(t, E);
      if ((dir > 0 && t >= TOTAL) || (dir < 0 && t <= 0)) {
        busy = false; if (dir < 0) root.classList.remove('on'); done && done(); return;
      }
      requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  }
  // ---------- Search: QWERTY keyboard, results under it, cabinet preview ----------
  const KEYS = ['1234567890', 'QWERTYUIOP', "ASDFGHJKL'", 'ZXCVBNM-.&'].map(r => r.split(''))
    .concat([[{ k: ' ', label: 'SPACE', span: 4 }, { k: 'DEL', label: 'DEL', span: 3 }, { k: 'CLEAR', label: 'CLEAR', span: 3 }]]);
  const KEY_W = 40, KEY_H = 30, GAP = 6, KB_X = LIST_X, KB_Y = 150, RES_Y = 360, RES_ROW = 28, RES_N = 5;
  const Q = { q: '', zone: 'keys', r: 1, c: 0, res: 0, results: GAMES.slice() };
  let search, keyEls = [], resEls;
  const cellX = c => KB_X + c * (KEY_W + GAP);
  function keyAt(r, c) { // bottom row keys span several columns
    if (r < 4) return KEYS[r][c];
    let col = 0; for (const k of KEYS[4]) { if (c < col + k.span) return k; col += k.span; } return KEYS[4][2];
  }
  function buildSearch() {
    search = document.createElement('div');
    search.id = 'arcSearch';
    search.style.cssText = 'position:absolute;inset:0;display:none';
    let html = `<div class="a m muted sband" style="left:26px;top:101px">ARCADE / SEARCH</div>
      <div class="sband" id="sqField" style="position:absolute;left:${KB_X}px;top:124px;width:${10 * KEY_W + 9 * GAP}px;height:18px;border-bottom:1px solid ${RED}">
        <span class="h" id="sqText" style="position:absolute;left:4px;top:0"></span></div>`;
    KEYS.forEach((row, r) => {
      html += `<div class="sband sk-row" style="position:absolute;left:0;top:${KB_Y + r * (KEY_H + GAP)}px;width:100%;height:${KEY_H}px">`;
      let col = 0;
      row.forEach(k => {
        const key = typeof k === 'string' ? { k, label: k, span: 1 } : k, w = key.span * KEY_W + (key.span - 1) * GAP;
        html += `<div class="skey m" data-r="${r}" data-c="${col}" style="position:absolute;left:${cellX(col)}px;top:0;width:${w}px;height:${KEY_H}px;
          border:1px solid #303d3f;background:#0b0b0d;text-align:center;line-height:${KEY_H - 2}px">${key.label}</div>`;
        col += key.span;
      });
      html += '</div>';
    });
    html += `<div class="a m muted sband" id="sqCount" style="left:26px;top:${RES_Y - 22}px"></div><div id="sqRes" class="sband"></div>`;
    search.innerHTML = html;
    root.append(search);
    keyEls = [...search.querySelectorAll('.skey')];
    resEls = search.querySelector('#sqRes');
  }
  function filter() {
    const q = Q.q.trim();
    Q.results = q ? GAMES.filter(g => g.toUpperCase().split(/[^A-Z0-9'&.-]+/).some(w => w.startsWith(q)) || g.toUpperCase().includes(q)) : GAMES.slice();
    Q.res = Math.min(Q.res, Math.max(0, Q.results.length - 1));
  }
  function renderSearch() {
    search.querySelector('#sqText').innerHTML = Q.q.replace(/ /g, '&nbsp;') +
      `<i style="display:inline-block;width:10px;height:14px;margin-left:2px;vertical-align:-2px;background:${RED};animation:blink 1s steps(1) infinite"></i>`;
    keyEls.forEach(el => {
      const k = keyAt(+el.dataset.r, +el.dataset.c), on = Q.zone === 'keys' && keyAt(Q.r, Q.c) === k;
      el.style.borderColor = on ? RED : '#303d3f'; el.style.background = on ? '#3a1511' : '#0b0b0d';
      el.style.color = on ? '#fff' : ''; el.style.boxShadow = on ? '0 0 8px #a4382c88' : 'none';
    });
    search.querySelector('#sqCount').textContent = `${Q.results.length} MATCH${Q.results.length === 1 ? '' : 'ES'}`;
    const first = Math.max(0, Math.min(Q.res - 2, Q.results.length - RES_N));
    resEls.innerHTML = Q.results.slice(first, first + RES_N).map((g, i) => {
      const [base, rest] = split(g), on = Q.zone === 'results' && first + i === Q.res;
      return `<div style="position:absolute;left:${LIST_X}px;top:${RES_Y + i * RES_ROW}px;width:${LIST_W}px;height:${RES_ROW}px;overflow:hidden;
        ${on ? 'background:linear-gradient(90deg,#3a1511 0%,#1d0a08 65%,rgba(0,0,0,0) 100%)' : ''}">
        ${on ? `<i style="position:absolute;left:0;top:0;bottom:0;width:4px;background:${RED}"></i>` : ''}
        <span class="h" style="position:absolute;left:14px;top:7px;white-space:nowrap;color:${on ? '#f3b5ab' : ''}">${base.toUpperCase()}
        <span class="m" style="color:var(--muted);margin-left:8px">${rest.toUpperCase()}</span></span>
        <i style="position:absolute;left:14px;right:0;bottom:0;height:1px;background:var(--rule)"></i></div>`;
    }).join('');
    showShot(Q.results[Q.zone === 'results' ? Q.res : 0]);
  }
  function type(k) {
    if (k === 'DEL') Q.q = Q.q.slice(0, -1);
    else if (k === 'CLEAR') Q.q = '';
    else if (Q.q.length < 24) Q.q += k;
    filter(); renderSearch();
  }
  function searchKey(key) {
    const A = key === 'Enter' || key === 'a', dir = { ArrowUp: [-1, 0], ArrowDown: [1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1] }[key];
    if (Q.zone === 'keys') {
      if (dir) {
        if (dir[0] === 1 && Q.r === 4 && Q.results.length) { Q.zone = 'results'; Q.res = 0; }
        else if (dir[1]) { // step over a spanning key as one key
          let c = Q.c, k = keyAt(Q.r, c);
          do { c = (c + dir[1] + 10) % 10; } while (keyAt(Q.r, c) === k && c !== Q.c);
          Q.c = c;
        } else Q.r = Math.max(0, Math.min(4, Q.r + dir[0]));
      } else if (A) { const k = keyAt(Q.r, Q.c); type(typeof k === 'string' ? k : k.k); return; }
      else if (/^[a-z0-9'&.\- ]$/i.test(key) && key !== 'a' && key !== 'b') { type(key.toUpperCase()); return; }
      else if (key === 'Backspace') { type('DEL'); return; }
    } else {
      if (key === 'ArrowUp' && Q.res === 0) Q.zone = 'keys';
      else if (key === 'ArrowUp') Q.res--;
      else if (key === 'ArrowDown') Q.res = Math.min(Q.results.length - 1, Q.res + 1);
      else if (A) { const g = Q.results[Q.res]; if (LISTG !== GAMES) setSection(0); sel = GAMES.indexOf(g); render(true); closeSearch(); return; }
    }
    renderSearch();
  }
  // Arcade list <-> Search: the cabinet stays; the list pushes out and the
  // field, key rows and results deal in row by row.
  let searching = false;
  function pushBands(outEls, inEls, dir, done) {
    busy = true;
    outEls.forEach(el => el.animate([{ opacity: 1, transform: 'none' }, { opacity: 0, transform: `translateX(${-40 * dir}px)` }],
      { duration: 180, easing: 'cubic-bezier(.4,0,1,1)', fill: 'forwards' }));
    setTimeout(() => {
      done();
      outEls.forEach(el => el.getAnimations().forEach(a => a.cancel()));
      inEls.forEach((el, i) => el.animate([{ opacity: 0, transform: `translateX(${32 * dir}px)` }, { opacity: 1, transform: 'none' }],
        { duration: 280, delay: i * 26, easing: 'cubic-bezier(.2,.8,.2,1)', fill: 'backwards' }));
      setTimeout(() => { busy = false; }, 280 + inEls.length * 26);
    }, 120);
  }
  const listParts = () => [root.querySelector('#arcLabel'), root.querySelector('#arcCount'), root.querySelector('#arcView')];
  function openSearch() {
    searching = true; Q.zone = 'keys'; filter(); renderSearch();
    pushBands(listParts(), [...search.querySelectorAll('.sband')], 1, () => {
      listParts().forEach(el => el.style.visibility = 'hidden'); search.style.display = 'block';
      document.getElementById('footArc').style.opacity = 0; document.getElementById('footSearch').style.opacity = 1;
    });
  }
  function closeSearch() {
    searching = false;
    pushBands([...search.querySelectorAll('.sband')], listParts(), -1, () => {
      search.style.display = 'none'; listParts().forEach(el => el.style.visibility = '');
      document.getElementById('footArc').style.opacity = 1; document.getElementById('footSearch').style.opacity = 0;
      showShot(LISTG[sel]);
    });
  }
  function key(k) {
    if (busy) return true;
    if (searching) { if (k === 'Escape' || k === 'b') { closeSearch(); return true; } searchKey(k); return true; }
    if (k === 'Tab') { toggleMode(); return true; }
    if (amode === 'hub') {
      if (k === 'ArrowRight') { atile = Math.min(2, atile + 1); hub.setFocus(atile); return true; }
      if (k === 'ArrowLeft') { atile = Math.max(0, atile - 1); hub.setFocus(atile); return true; }
      if (k === 'Enter' || k === 'a') { openSection(atile); return true; }
      if (k === 'Escape' || k === 'b' || k === 'Backspace') return false;
      return true;
    }
    if (k === 'y' || k === 'Y') { openSearch(); return true; }
    return false;
  }

  function init(stage) {
    build(stage);
    buildSearch();
    // The card's outline and face for the zoom, above the page.
    outline = document.createElement('div');
    outline.style.cssText = `position:absolute;left:${CARD.x - 1}px;top:${CARD.y - 1}px;width:${CARD.w + 2}px;height:${CARD.h + 2}px;
      border:3px solid ${RED};border-radius:10px;box-shadow:0 0 14px #a4382caa;transform-origin:50% 50%;opacity:0;pointer-events:none`;
    face = document.createElement('div');
    face.style.cssText = `position:absolute;left:${CARD.x}px;top:${CARD.y}px;width:${CARD.w}px;height:${CARD.h}px;border-radius:8px;
      background:url(launcher-arcade-cabinet.png) -${CARD.x}px -${CARD.y}px;transform-origin:50% 50%;opacity:0;pointer-events:none`;
    stage.insertBefore(face, document.getElementById('frame'));
    stage.insertBefore(outline, document.getElementById('frame'));
  }
  return { init, move, run, zoomFrame, setMode: m => { amode = m; applyAMode(true); }, GAMES, shotFor, key, openSearch, get searching() { return searching; }, get busy() { return busy; }, set sel(v) { sel = v; render(true); }, get sel() { return sel; } };
})();
