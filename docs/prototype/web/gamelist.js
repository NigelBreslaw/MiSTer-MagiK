// The system page (SNES, C64, Game Boy...). The device with the selected
// game's screenshot stays on the right; the left side is the hub (identity and
// the Games / Recent / Favourites tiles) or the game list, and Select toggles
// between them. It is entered from the level's card by the same kind of zoom
// as the Arcade card: the card zooms past the edges while the device rises out
// of it and the hub deals in. The device is generic: a TV for consoles, a
// monitor for computers and a handheld for handhelds.
window.GLIST = (() => {
  const clamp01 = v => v < 0 ? 0 : v > 1 ? 1 : v;
  const inOut = t => t < .5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
  const out = t => 1 - Math.pow(1 - t, 4);
  const win = (t, at, dur) => clamp01((t - at) / dur);

  // Geometry (960x540): list on the left, device on the right. Every device
  // is 483x519 with the cabinet's 320x320 screen opening at (82, 61).
  const LIST_X = 26, LIST_W = 462, VIEW_TOP = 124, VIEW_BOTTOM = 494, ROW = 36, FOCUS_ROW = 3;
  const DEV = { x: 490, y: 35, w: 483, h: 519 };
  const SCREEN = { x: DEV.x + 82, y: DEV.y + 61, s: 320 };
  // The part of the device that the chosen tile turns into (the screen and its bezel).
  const REGION = { x: 117, y: 46, w: 250, h: 350 };
  const DEVICE = { pad: 'device-tv.png', computer: 'device-monitor.png', handheld: 'device-handheld.png' };
  const SHOT = { pad: { w: 256, h: 224, px: 1 }, computer: { w: 320, h: 200, px: 1 }, handheld: { w: 320, h: 288, px: 2 } };

  const TITLES = {
    pad: ['Super Mario World', 'The Legend of Zelda: A Link to the Past', 'Super Metroid', 'Donkey Kong Country', 'F-Zero',
      'Chrono Trigger', 'Final Fantasy VI', 'Super Mario Kart', 'Street Fighter II Turbo', 'Secret of Mana', 'Contra III: The Alien Wars',
      'Mega Man X', 'Star Fox', "Kirby's Dream Land 3", 'EarthBound', "Yoshi's Island", 'Super Castlevania IV', 'Pilotwings',
      'Super Mario RPG', 'Killer Instinct', 'Actraiser', 'Super Ghouls n Ghosts', 'Demon\'s Crest', 'Zombies Ate My Neighbors',
      'Super Bomberman', 'Populous', 'R-Type III', 'Axelay', 'Plok', 'Sunset Riders', 'Super Punch-Out!!', 'Terranigma',
      'Lufia II', 'Illusion of Gaia', 'Breath of Fire II', 'Top Gear 2'],
    computer: ['Impossible Mission', 'Boulder Dash', 'Maniac Mansion', 'Elite', 'Turrican II', 'The Great Giana Sisters', 'Paradroid',
      'Uridium', 'Wizball', 'Bubble Bobble', 'Arkanoid', 'Lemmings', 'Speedball 2', 'Cannon Fodder', 'Sensible Soccer', 'Populous',
      'Prince of Persia', 'The Secret of Monkey Island', 'Head Over Heels', 'Manic Miner', 'Jet Set Willy', 'Dizzy', 'Rick Dangerous',
      'Lotus Esprit Turbo Challenge', 'Shadow of the Beast', 'Another World', 'Defender of the Crown', 'Zak McKracken', 'Katakis',
      'International Karate', 'Last Ninja 2', 'Creatures', 'Microprose Soccer', 'Rainbow Islands', 'Pirates!', 'Sim City'],
    handheld: ['Tetris', 'Pokemon Red', "Link's Awakening", 'Super Mario Land', "Kirby's Dream Land", 'Metroid II: Return of Samus',
      'Wario Land', 'Donkey Kong', 'Mega Man V', "Castlevania: The Adventure", 'Dr. Mario', 'Final Fantasy Adventure', 'Oracle of Ages',
      'Pokemon Gold', 'Super Mario Land 2', 'Gargoyle\'s Quest', 'Kid Icarus: Of Myths and Monsters', 'Balloon Kid', 'Bionic Commando',
      'Contra: The Alien Wars', 'Ducktales', 'F-1 Race', 'Kirby\'s Pinball Land', 'Operation C', 'Probotector', 'R-Type', 'Solar Striker',
      'Trip World', 'Wave Race', 'Zelda: Oracle of Seasons', 'Harvest Moon GB', 'Mole Mania', 'Space Invaders', 'Track & Field'],
  };

  const hex = h => [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16));
  const rgba = (h, a) => `rgba(${hex(h).join(',')},${a})`;
  const mixHex = (a, b, k) => { const x = hex(a), y = hex(b); return `rgb(${x.map((v, i) => Math.round(v + (y[i] - v) * k)).join(',')})`; };
  const split = g => { const i = g.search(/ [(\[:]/); return i < 0 ? [g, ''] : [g.slice(0, i), g.slice(i)]; };

  // ---- placeholder screenshots: a plausible retro game screen, seeded by the title ----
  const shotCache = {}, shotCanv = {};
  function shotSrc(title, kind) {
    const key = kind + title;
    if (shotCache[key]) return shotCache[key];
    let h = 2166136261; for (const ch of title) h = Math.imul(h ^ ch.charCodeAt(0), 16777619);
    const rnd = () => (h = Math.imul(h ^ (h >>> 15), 2246822507) >>> 0, (h % 10000) / 10000);
    const S = SHOT[kind], px = S.px, w = S.w / px, hh = S.h / px;
    const c = document.createElement('canvas'); c.width = w; c.height = hh;
    const g = c.getContext('2d');
    const hue = Math.floor(rnd() * 360);
    const col = (hh2, s, l) => `hsl(${(hue + hh2) % 360},${s}%,${l}%)`;
    const mono = kind === 'handheld', pal = mono ? ['#0f380f', '#306230', '#8bac0f', '#9bbc0f'] : null;
    const bands = 7;
    for (let i = 0; i < bands; i++) {                                   // banded sky
      g.fillStyle = mono ? pal[3 - Math.min(3, i >> 1)] : col(0, 55, 28 + i * 6); g.fillRect(0, i * hh * .5 / bands, w, hh * .5 / bands + 1);
    }
    g.fillStyle = mono ? pal[2] : col(40, 40, 26);                     // far hills
    for (let x = 0; x < w; x += 4) g.fillRect(x, hh * .5 - 6 - Math.sin(x * .05 + rnd() * 2) * 8 - rnd() * 3, 4, 40);
    g.fillStyle = mono ? pal[1] : col(140, 50, 24); g.fillRect(0, hh * .68, w, hh);      // ground
    g.fillStyle = mono ? pal[0] : col(150, 50, 15);
    for (let x = 0; x < w; x += 16) g.fillRect(x + ((x >> 4) & 1) * 8, hh * .68, 8, 4);
    for (let i = 0; i < 4; i++) {                                       // platforms
      const px0 = Math.floor(rnd() * (w - 60)), py = Math.floor(hh * (.32 + rnd() * .3));
      g.fillStyle = mono ? pal[0] : col(180, 60, 40); g.fillRect(px0, py, 40 + rnd() * 30, 6);
      g.fillStyle = mono ? pal[3] : col(180, 60, 60); g.fillRect(px0, py, 40, 2);
    }
    for (let i = 0; i < 6; i++) { g.fillStyle = mono ? pal[3] : '#ffd23a'; g.fillRect(20 + rnd() * (w - 40), hh * (.25 + rnd() * .35), 5, 5); }
    const sx = Math.floor(w * (.2 + rnd() * .5)), sy = Math.floor(hh * .68) - 22;     // the hero
    g.fillStyle = mono ? pal[0] : '#d03030'; g.fillRect(sx, sy, 12, 8);
    g.fillStyle = mono ? pal[0] : '#f0c090'; g.fillRect(sx + 2, sy + 8, 8, 6);
    g.fillStyle = mono ? pal[0] : '#3050c0'; g.fillRect(sx + 1, sy + 14, 10, 8);
    g.fillStyle = mono ? pal[3] : '#fff'; g.font = '8px Xerxes, monospace'; g.textBaseline = 'top';
    g.fillText(title.toUpperCase().slice(0, 22), 6, 5);                 // HUD line
    g.fillText('SCORE 004250', 6, 15);
    if (px === 1) { shotCanv[key] = c; return (shotCache[key] = c.toDataURL()); }
    const big = document.createElement('canvas'); big.width = S.w; big.height = S.h;
    const bg = big.getContext('2d'); bg.imageSmoothingEnabled = false; bg.drawImage(c, 0, 0, S.w, S.h);
    shotCanv[key] = big;
    return (shotCache[key] = big.toDataURL());
  }
  const shotCanvas = (title, kind) => { shotSrc(title, kind); return shotCanv[kind + title]; };

  // ---- the page ----
  // One page per system. The device (with the selected game's screenshot in its
  // screen) stays on the right; the left side is either the hub (identity and
  // the Games / Recent / Favourites tiles) or the game list, and Select
  // toggles between them. The page lands on the hub.
  let root, list, cab, screen, shots, outline, face, hdr, footHub, footList, frontShot = 0, hub = null;
  let ctx = null, sel = 0, games = [], kind = 'pad', accent = '#5a71e7', active = false, busy = false, ZMAX = 8;
  let mode = 'hub', section = 0, tile = 0;
  const TOTAL = 1000;
  const SECTIONS = ['GAMES', 'RECENT', 'FAVOURITES'];

  function build(stage) {
    root = document.createElement('div'); root.id = 'glist'; root.className = 'page';
    root.innerHTML = `
      <div id="glLabel" class="lpart"><div class="a m muted" id="glLabelT" style="left:26px;top:101px"></div>
        <div class="a m muted" id="glCount" style="left:${LIST_X + LIST_W - 200}px;top:101px;width:200px;text-align:right"></div></div>
      <div id="glView" class="lpart" style="position:absolute;left:${LIST_X}px;top:${VIEW_TOP}px;width:${LIST_W}px;height:${VIEW_BOTTOM - VIEW_TOP}px;overflow:hidden">
        <div id="glFocus" style="position:absolute;left:0;top:${FOCUS_ROW * ROW}px;width:100%;height:${ROW}px"><i style="position:absolute;left:0;top:0;bottom:0;width:4px"></i></div>
        <div id="glList" style="position:absolute;left:0;top:0;width:100%;transition:transform .14s cubic-bezier(.2,.8,.2,1)"></div>
      </div>
      <div id="glHub"></div>
      <div id="glDev" style="position:absolute;left:${DEV.x}px;top:${DEV.y}px;width:${DEV.w}px;height:${DEV.h}px;transform-origin:0 0">
        <img id="glDevImg" alt="" style="position:absolute;left:0;top:0;width:${DEV.w}px;height:${DEV.h}px">
        <i style="position:absolute;left:0;top:${DEV.h}px;width:${DEV.w}px;height:140px;background:#000"></i>
        <div id="glScreen" style="position:absolute;left:${SCREEN.x - DEV.x}px;top:${SCREEN.y - DEV.y}px;width:${SCREEN.s}px;height:${SCREEN.s}px;overflow:hidden;background:#000">
          <img class="shot" style="position:absolute;inset:0;width:100%;height:100%;object-fit:none;image-rendering:pixelated;transition:opacity .13s linear">
          <img class="shot" style="position:absolute;inset:0;width:100%;height:100%;object-fit:none;image-rendering:pixelated;transition:opacity .13s linear;opacity:0">
        </div>
      </div>
      <div id="glHdr" style="opacity:0">
        <div style="position:absolute;left:0;top:0;width:960px;height:77px;background:#000"></div>
        <div class="a h" style="left:26px;top:21px">MISTER MAGIK</div>
        <div class="a h" style="left:874px;top:26px">17:33</div>
        <div style="position:absolute;left:26px;top:76px;width:908px;height:1px;background:var(--rule)"></div>
        <div style="position:absolute;left:0;top:500px;width:960px;height:40px;background:#000"></div>
        <div style="position:absolute;left:26px;top:500px;width:908px;height:1px;background:var(--rule)"></div>
      </div>
      <div id="glFootHub" style="opacity:0">
        <div class="a m foot" style="left:30px;top:518px">A &nbsp;OPEN</div>
        <div class="a m foot" style="left:130px;top:518px">B &nbsp;BACK</div>
        <div class="a m muted" style="left:600px;top:518px">SELECT &nbsp;GAME LIST</div>
      </div>
      <div id="glFootList" style="opacity:0">
        <div class="a m foot" style="left:30px;top:518px">A &nbsp;PLAY</div>
        <div class="a m foot" style="left:130px;top:518px">B &nbsp;BACK</div>
        <div class="a m muted" style="left:600px;top:518px">SELECT &nbsp;OVERVIEW</div>
      </div>`;
    stage.append(root);
    list = root.querySelector('#glList'); cab = root.querySelector('#glDev'); screen = root.querySelector('#glScreen');
    hdr = root.querySelector('#glHdr'); footHub = root.querySelector('#glFootHub'); footList = root.querySelector('#glFootList');
    shots = [...root.querySelectorAll('.shot')];
    outline = document.createElement('div'); outline.style.cssText = 'position:absolute;opacity:0;pointer-events:none;transform-origin:50% 50%;box-sizing:border-box;border-radius:10px';
    stage.append(outline);
  }

  const listParts = () => [...root.querySelectorAll('.lpart')];
  const hubParts = () => hub ? hub.bands : [];
  const titlesFor = s => {
    const all = TITLES[s.icon || 'pad'];
    return section === 1 ? all.slice(0, Math.max(1, s.recent)) : section === 2 ? all.slice(3, 3 + Math.max(1, s.favs)) : all;
  };
  // Fill the list for the current section.
  function fillList() {
    const s = ctx.sys;
    games = titlesFor(s); sel = Math.min(FOCUS_ROW, games.length - 1);
    root.querySelector('#glLabelT').textContent = s.name + (section === 1 ? ' / RECENT' : section === 2 ? ' / FAVOURITES' : '');
    root.querySelector('#glCount').textContent = `${section === 1 ? s.recent : section === 2 ? s.favs : s.games} ${section === 0 ? 'GAMES' : 'TITLES'}`;
    list.innerHTML = games.map((g, i) => {
      const [base, rest] = split(g);
      return `<div class="arow" style="position:absolute;left:0;top:${i * ROW}px;width:100%;height:${ROW}px">
        <span class="h" style="position:absolute;left:14px;top:11px;max-width:${LIST_W - 28}px;overflow:hidden">${base.toUpperCase()}
        <span class="m" style="color:var(--muted);margin-left:8px">${rest.toUpperCase()}</span></span>
        <i style="position:absolute;left:14px;right:0;bottom:0;height:1px;background:var(--rule)"></i></div>`;
    }).join('');
  }

  function setup(c) {
    ctx = c; const s = c.sys;
    kind = s.icon || 'pad'; accent = s.acc || '#5a71e7'; mode = c.mode || 'hub'; section = 0; tile = 0;
    const focus = root.querySelector('#glFocus');
    focus.style.background = `linear-gradient(90deg,${rgba(accent, .3)} 0%,${rgba(accent, .1)} 65%,rgba(0,0,0,0) 100%)`;
    focus.firstElementChild.style.cssText += `;background:${accent};box-shadow:0 0 8px ${rgba(accent, .8)}`;
    root.querySelector('#glDevImg').src = c.deviceSrc || DEVICE[kind];
    fillList();
    // the hub
    const holder = root.querySelector('#glHub'); holder.innerHTML = '';
    hub = HUB.build({
      title: s.full, subtitle: s.hubSubtitle || `${s.maker} &nbsp;/&nbsp; ${s.year} &nbsp;/&nbsp; ${s.gen}`, accent,
      count: `${s.games} GAMES READY TO PLAY`,
      tiles: [{ n: s.games, label: 'GAMES' }, { n: s.recent, label: 'RECENT' }, { n: s.favs, label: 'FAVOURITES' }],
      captions: [`BROWSE ALL ${s.games} ${s.full} GAMES`, s.recent ? 'PICK UP WHERE YOU LEFT OFF' : 'NOTHING PLAYED YET', s.favs ? `${s.favs} SAVED FAVOURITE${s.favs === 1 ? '' : 'S'}` : 'NO FAVOURITES YET'],
    });
    holder.append(hub.el); hub.setFocus(0, false);
    outline.style.left = (c.rect.x - 1) + 'px'; outline.style.top = (c.rect.y - 1) + 'px';
    outline.style.width = (c.rect.w + 2) + 'px'; outline.style.height = (c.rect.h + 2) + 'px';
    outline.style.border = `3px solid ${accent}`; outline.style.boxShadow = `0 0 14px ${rgba(accent, .7)}`;
    const cx = c.rect.x + c.rect.w / 2, cy = c.rect.y + c.rect.h / 2;
    ZMAX = 1.15 * Math.max(Math.max(cx, 960 - cx) / (c.rect.w / 2), Math.max(cy, 540 - cy) / (c.rect.h / 2));
    // the chosen card's face, scaling with the window and fading out
    if (face) face.remove();
    face = document.createElement('div');
    face.style.cssText = `position:absolute;left:${c.rect.x}px;top:${c.rect.y}px;width:${c.rect.w}px;height:${c.rect.h}px;border-radius:8px;background:url(${c.faceUrl}) center/100% 100%;transform-origin:50% 50%;opacity:0;pointer-events:none`;
    document.getElementById('stage') ? document.getElementById('stage').append(face) : root.append(face);
    applyMode(true);
    render(true);
  }

  // Show one side of the left panel and its footer.
  function applyMode(instant) {
    const showList = mode === 'list';
    listParts().forEach(el => { el.style.visibility = showList ? '' : 'hidden'; });
    hubParts().forEach(el => { el.style.visibility = showList ? 'hidden' : ''; });
    footHub.style.display = showList ? 'none' : ''; footList.style.display = showList ? '' : 'none';
    if (!instant) (showList ? footList : footHub).style.opacity = 1;
  }

  function render(instant) {
    if (instant) list.style.transition = 'none';
    list.style.transform = `translateY(${(FOCUS_ROW - sel) * ROW}px)`;
    [...list.children].forEach((row, i) => {
      row.querySelector('.h').style.color = i === sel ? mixHex(accent, '#ffffff', .62) : '';
      row.classList.toggle('cur', i === sel);
      row.style.opacity = Math.abs(i - sel) > 7 ? 0 : 1;
    });
    if (instant) { list.offsetHeight; list.style.transition = ''; }
    showShot(games[sel]);
  }
  function showShot(game) {
    const src = game ? shotSrc(game, kind) : '';
    if (shots[frontShot].getAttribute('src') === src && shots[frontShot].style.opacity !== '0') return;
    const next = shots[1 - frontShot];
    if (src) next.src = src; else next.removeAttribute('src');
    next.style.opacity = src ? 1 : 0; shots[frontShot].style.opacity = 0; frontShot = 1 - frontShot;
  }
  function move(d) { const n = Math.max(0, Math.min(games.length - 1, sel + d)); if (n !== sel) { sel = n; render(false); } }

  // Slide one side of the left panel out and the other in; the device stays.
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
  function toggle() {
    const toList = mode === 'hub';
    const outEls = toList ? hubParts() : listParts(), inEls = toList ? listParts() : hubParts();
    if (toList) render(true);
    pushBands(outEls, inEls, toList ? 1 : -1, () => { mode = toList ? 'list' : 'hub'; applyMode(); });
  }
  // A on a tile: that section's list.
  function openSection(i) {
    const s = ctx.sys; if (![s.games, s.recent, s.favs][i]) return;
    section = i; fillList(); toggle();
  }

  // The zoom: the chosen card scales past the edges while the device rises out
  // of it (clipped to the growing window) and the left side deals in.
  function zoomFrame(t) {
    const R = ctx.rect, cx = R.x + R.w / 2, cy = R.y + R.h / 2;
    const z = Math.exp(Math.log(ZMAX) * inOut(win(t, 0, 760)));
    outline.style.transform = `scale(${z})`; outline.style.opacity = Math.max(0, 1 - Math.pow(win(t, 0, 760), 2) * 1.25);
    face.style.transform = `scale(${z})`; face.style.opacity = t > 0 ? 1 - win(t, 60, 200) : 0;
    const pc = inOut(win(t, 80, 760));
    const s0 = R.w / REGION.w, x0 = R.x - REGION.x * s0, y0 = R.y - REGION.y * s0;
    const tx = (x0 - DEV.x) * (1 - pc), ty = (y0 - DEV.y) * (1 - pc), sc = s0 + (1 - s0) * pc;
    cab.style.transform = `translate(${tx}px,${ty}px) scale(${sc})`;
    const ww = R.w * z, wh = R.h * z, wx = cx - ww / 2, wy = cy - wh / 2;
    const L = (wx - DEV.x - tx) / sc, T = (wy - DEV.y - ty) / sc;
    const Rr = DEV.w - (wx + ww - DEV.x - tx) / sc, B = DEV.h - (wy + wh - DEV.y - ty) / sc;
    cab.style.clipPath = z >= ZMAX ? 'none' : `inset(${T}px ${Rr}px ${B}px ${L}px round ${8 * z / sc}px)`;
    cab.style.opacity = t > 0 ? 1 : 0;
    // the level you came from leaves; the page's own header takes over
    const a = 1 - win(t, 40, 240);
    ctx.fadeEls.forEach(e => { e.style.opacity = a; });
    hdr.style.opacity = win(t, 260, 200);
    // the left side deals in, then the first screenshot lights up
    const rows = mode === 'hub' ? hubParts() : [root.querySelector('#glLabel'), root.querySelector('#glFocus'), ...list.children];
    rows.forEach((r, i) => {
      const k = out(win(t, 500 + Math.min(i, 10) * 26, 280));
      const isRow = r.classList && r.classList.contains('arow');
      r.style.opacity = isRow && Math.abs(i - 2 - sel) > 7 ? 0 : k;
      if (isRow || mode === 'hub') r.style.translate = `${Math.round(40 * (1 - k))}px 0`;
    });
    screen.style.opacity = win(t, 760, 200);
    (mode === 'hub' ? footHub : footList).style.opacity = win(t, 640, 220);
  }

  function run(dir, c, slow, done) {
    if (busy) return;
    if (dir > 0) setup(c);
    busy = true; active = true; root.classList.add('on');
    let t = dir > 0 ? 0 : TOTAL, last = performance.now();
    zoomFrame(t);
    const step = now => {
      t = Math.max(0, Math.min(TOTAL, t + dir * (now - last) / ((typeof slow === 'number' && slow) || 1))); last = now;
      zoomFrame(t);
      if ((dir > 0 && t >= TOTAL) || (dir < 0 && t <= 0)) {
        busy = false;
        if (dir < 0) close();
        done && done(); return;
      }
      requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  }
  // Put the level back exactly as it was and hide the page.
  function close() {
    active = false; root.classList.remove('on');
    if (face) { face.remove(); face = null; }
    outline.style.opacity = 0; hdr.style.opacity = 0;
    [footHub, footList].forEach(f => { f.style.opacity = 0; });
    [...listParts(), ...hubParts()].forEach(el => { el.style.opacity = ''; el.style.translate = ''; });
    if (ctx) ctx.fadeEls.forEach(e => { e.style.opacity = 1; e.style.transform = ''; });
  }
  function key(k) {
    if (busy) return true;
    const back = k === 'Escape' || k === 'b' || k === 'Backspace';
    if (k === 'Tab') { toggle(); return true; }
    if (back) { run(-1, ctx, typeof slow === 'number' ? slow : 1, ctx.onClosed); return true; }
    if (mode === 'hub') {
      if (k === 'ArrowRight') { tile = Math.min(2, tile + 1); hub.setFocus(tile); }
      else if (k === 'ArrowLeft') { tile = Math.max(0, tile - 1); hub.setFocus(tile); }
      else if (k === 'Enter' || k === 'a') openSection(tile);
    } else {
      if (k === 'ArrowDown') move(1); else if (k === 'ArrowUp') move(-1);
      else if (k === 'ArrowRight') move(9); else if (k === 'ArrowLeft') move(-9);
    }
    return true;
  }
  function init(stage) { build(stage); }
  // One frame of the page for review (no animation).
  function preview(c, ms) { setup(c); active = true; root.classList.add('on'); zoomFrame(ms); }
  return { init, run, key, zoomFrame, setup, close, preview, shotCanvas, toggle, TITLES, get mode() { return mode; }, get active() { return active; }, get busy() { return busy; } };
})();
