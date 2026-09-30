// CRT (240p) variant of the Settings design, drawn on the native 640x240
// raster and shown 2x vertically (4:3). Text is Spleen 6x12 from the repo's
// BDF, doubled horizontally exactly like the CRT card launcher.
window.CRT = (() => {
  const W = 640, H = 240;
  const C = { cream: '#eee8d5', muted: '#8f9796', dim: '#5d6564', rule: '#303d3f', violet: '#9c86e7',
    focusA: '#221843', focusB: '#120d24', panel: '#0b0913', panelEdge: '#4a3d73', btn: '#3a3350', btnOn: '#1d1535' };
  // Launcher CRT metrics (responsive.rs, 240p): margins, header and footer rules.
  const L = 38, R = 602, MY = 12, HEADER_RULE = 30, FOOTER_RULE = 208, LIST_TOP = 56;
  const ROW = 16, GROUP_GAP = 6, TEXT_DY = 2;

  let canvas, ctx, ready = false, slow = 1;
  const img = {};

  // ---------- Spleen 6x12 BDF -> per-colour glyph atlases ----------
  const glyphs = new Map();
  async function loadFont() {
    const lines = (await (await fetch('spleen-6x12.bdf')).text()).split('\n');
    let code = null, bbx = null, bits = null;
    for (const line of lines) {
      if (line.startsWith('ENCODING ')) code = +line.slice(9);
      else if (line.startsWith('BBX ')) bbx = line.slice(4).split(' ').map(Number);
      else if (line === 'BITMAP') bits = [];
      else if (line === 'ENDCHAR') { glyphs.set(code, { bbx, bits }); bits = null; }
      else if (bits) bits.push(parseInt(line, 16));
    }
  }
  const atlases = new Map();
  function atlas(color, sx, sy) {
    const key = color + sx + sy;
    if (atlases.has(key)) return atlases.get(key);
    const a = document.createElement('canvas'); a.width = 96 * 6 * sx; a.height = 12 * sy;
    const g = a.getContext('2d'); g.fillStyle = color;
    for (let c = 32; c < 128; c++) {
      const glyph = glyphs.get(c); if (!glyph) continue;
      const [w, h, xo, yo] = glyph.bbx, top = 9 - (h + yo);
      glyph.bits.forEach((row, r) => {
        for (let x = 0; x < w; x++) if (row & (0x80 >> x))
          g.fillRect(((c - 32) * 6 + xo + x) * sx, (top + r) * sy, sx, sy);
      });
    }
    atlases.set(key, a); return a;
  }
  // sx=2 is the launcher's 12x12 cell; sx=1 is native 6x12 for long text.
  function text(str, x, y, color = C.cream, alpha = 1, sx = 2) {
    if (alpha <= 0) return;
    const a = atlas(color, sx, 1), cw = 6 * sx;
    ctx.globalAlpha = alpha;
    for (let i = 0; i < str.length; i++) {
      const c = str.charCodeAt(i); if (c < 32 || c > 127) continue;
      ctx.drawImage(a, (c - 32) * cw, 0, cw, 12, Math.round(x) + i * cw, Math.round(y), cw, 12);
    }
    ctx.globalAlpha = 1;
  }
  const tw = (str, sx = 2) => str.length * 6 * sx;
  function rect(x, y, w, h, color, alpha = 1) {
    if (alpha <= 0) return; ctx.globalAlpha = alpha; ctx.fillStyle = color;
    ctx.fillRect(Math.round(x), Math.round(y), Math.round(w), Math.round(h)); ctx.globalAlpha = 1;
  }

  // ---------- easing ----------
  const clamp01 = v => v < 0 ? 0 : v > 1 ? 1 : v;
  const inOut = t => t < .5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
  const out = t => 1 - Math.pow(1 - t, 4);
  const win = (t, at, dur) => clamp01((t - at) / dur);
  const lerp = (a, b, p) => a + (b - a) * p;

  // ---------- content ----------
  const STEPS = ['OFF', '1 MIN', '2 MIN', '3 MIN', '4 MIN', '5 MIN', '6 MIN', '7 MIN', '8 MIN', '9 MIN', '10 MIN'];
  const ROWS = [
    { l: 'DISPLAY RESOLUTION', v: 'CRT 240P 60HZ NTSC', combo: 'display', g: 0 },
    { l: 'SCREEN ORIENTATION', v: 'NORMAL', combo: 'orientation', g: 0 },
    { l: 'REDUCE MOTION', toggle: true, g: 0 },
    { l: 'START AFTER', stepper: true, g: 1 },
    { l: 'PREVIEW SCREENSAVER', v: 'SHOW NOW', link: true, g: 1 },
    { l: 'EXIT TO MISTER', v: 'UNTIL REBOOT', g: 2 },
    { l: 'REFRESH DATABASE', v: 'CHANGED SYSTEMS', g: 2 },
    { l: 'ABOUT', v: 'MAGIK', link: true, g: 2 },
  ];
  // Rows sit in groups separated by a gap with a faint rule; no headings at 240p.
  ROWS.forEach((r, i) => { r.y = LIST_TOP + i * ROW + r.g * GROUP_GAP; });
  const CHOICES = {
    display: ['1280X720 (16:9)', '1366X768 (16:9)', '1920X1080 (16:9)', '1920X1200 (16:10)', '2048X1536 (4:3)',
      '2560X1440 (16:9)', null, 'CRT 240P 60HZ NTSC', 'CRT 288P 50HZ PAL', 'CRT 480P 60HZ NTSC', 'CRT 576P 50HZ PAL'],
    orientation: ['NORMAL', 'MONITOR RIGHT', 'MONITOR LEFT'],
  };
  const ABOUT = [
    { l: 'BUILD', v: '0.2.112 DEV', info: true },
    { l: 'GAME DATABASE', v: 'V40 26 SEP', info: true },
    { l: 'KERNEL', v: '6.18.38-MISTER', info: true },
    { l: 'LICENSES', v: '12', link: true, gap: true },
  ];
  ABOUT.forEach((r, i) => { r.y = LIST_TOP + i * ROW + (r.gap ? GROUP_GAP : 0); });
  const LICENSES = [['MISTER MAGIK', 'GPL-3.0'], ['FFMPEG', 'LGPL-2.1'], ['SLINT', 'GPL-3.0'], ['PRESS START 2P', 'OFL-1.1'],
    ['COMMERCIAL FONTS', 'LICENSED'], ['JERSEY 25', 'OFL-1.1'], ['JERSEY 15', 'OFL-1.1'], ['SPLEEN', 'BSD-2'],
    ['TERMINUS', 'OFL-1.1'], ['RUST STANDARD LIBRARY', 'MIT'], ['ZLIB', 'ZLIB'], ['LIBPNG', 'LIBPNG']];
  const LIC_PAGE = 9, TEXT_VIEW = 12;
  let TEXT_LINES = [];

  // ---------- state ----------
  const S = { scr: 'launcher', sel: 0, reduce: false, step: 5, lic: 0, textTop: 0, dd: null, anim: null,
    card: 'settings', arc: 5, arcScroll: 5, shot: null, shotPrev: null, shotAt: 0 };

  // ---------- geometry for the card zoom ----------
  // Settings card on the CRT launcher (raw 640x240), centred at (319.5, 109.5).
  const CARD = { x: 239, y: 54, w: 162, h: 112 };
  const CCX = CARD.x + CARD.w / 2, CCY = CARD.y + CARD.h / 2;
  // Backdrop asset 412x374 display pixels; raw rows are half height on 240p.
  // Card crop in the 816x1142 render: (204, 285.5) 408x571; asset crop (312, 327).
  const card_sx = CARD.w / 408, card_sy = CARD.h / 571;
  const COG_START = { x: CARD.x + (312 - 204) * card_sx, y: CARD.y + (327 - 285.5) * card_sy, sx: card_sx, sy: card_sy, a: 1 };
  // Watermark: 300 display px wide on the right, bleeding off the edge.
  const COG_REST = { x: 352, y: 50, sx: 300 / 412, sy: 150 / 374, a: 0.3 };
  const ZMAX = 7;

  function drawCog(p) {
    if (!img.cog || p.a <= 0) return;
    ctx.globalAlpha = p.a;
    ctx.imageSmoothingEnabled = true;
    ctx.drawImage(img.cog, p.x, p.y, 412 * p.sx, 374 * p.sy);
    ctx.imageSmoothingEnabled = false;
    ctx.globalAlpha = 1;
  }
  function roundRectPath(x, y, w, h, r) {
    ctx.beginPath(); ctx.moveTo(x + r, y); ctx.arcTo(x + w, y, x + w, y + h, r); ctx.arcTo(x + w, y + h, x, y + h, r);
    ctx.arcTo(x, y + h, x, y, r); ctx.arcTo(x, y, x + w, y, r); ctx.closePath();
  }

  // ---------- Arcade (240p): full-screen screenshot backdrop ----------
  // The selected game fills the TV at its own shape: a 320x240 game is the
  // whole 640x240 raster (every game pixel doubled across); a vertical game
  // is pillarboxed. A fixed scrim (dim + left gradient + top/bottom bands)
  // keeps the list readable while the art stays bright on the right.
  const ARC = { L: 38, R: 400, TOP: 56, BOTTOM: 200, FOCUS: 3, RED: '#e7695a' };
  const CARD_FACE = { x: 242, y: 57, w: 155, h: 105 };     // inside the card border (raw)
  const fullGames = () => window.ARCADE.GAMES;
  const games = () => S.arcList || fullGames();
  const ARC_RECENT = ['720 Degrees (rev 4)', '1942', 'Ms. Pac-Man', 'Out Run (sitdown/upright, Rev B)', 'Street Fighter II: The World Warrior (World 910522)'];
  const arcFavs = () => fullGames().filter(g => /^(1942|720|Action Fighter)/.test(g));
  // The Arcade "system" that the hub shows, in the same shape as a real system.
  const ARC_SYS = { name: 'ARCADE', full: 'ARCADE', maker: 'MANY MAKERS', year: '1971-2005', gen: 'COIN-OP', media: 'COIN-OP', ports: 2, games: 999, recent: 5, favs: 3, acc: '#e7695a' };
  // Full-screen rect of a shot in raster pixels (display px wide, rows halved).
  function shotRect(name) {
    const im = img[name], f = Math.min(W / im.width, (H * 2) / im.height);
    const w = im.width * f, h = im.height * f / 2;
    return { x: (W - w) / 2, y: (H - h) / 2, w, h };
  }
  // Draw the crossfading backdrop into a destination rect (full screen at rest,
  // the card face during the zoom). p maps the full-screen rect into it.
  function drawBackdrop(alpha = 1, into = null) {
    const draw = (name, a) => {
      if (!name || a <= 0) return;
      let r = shotRect(name);
      if (into) r = { x: into.x + r.x * into.w / W, y: into.y + r.y * into.h / H, w: r.w * into.w / W, h: r.h * into.h / H };
      ctx.globalAlpha = a; ctx.imageSmoothingEnabled = true;
      ctx.drawImage(img[name], r.x, r.y, r.w, r.h);
      ctx.imageSmoothingEnabled = false; ctx.globalAlpha = 1;
    };
    const k = clamp01((performance.now() - S.shotAt) / (130 * slow));
    draw(S.shotPrev, alpha * (1 - k)); draw(S.shot, alpha * k);
  }
  // The scrim is one precomputed 640x240 alpha mask on the device.
  let scrimCanvas = null;
  function scrim(amount = 1, extraDim = 0) {
    if (!scrimCanvas) {
      scrimCanvas = document.createElement('canvas'); scrimCanvas.width = W; scrimCanvas.height = H;
      const g = scrimCanvas.getContext('2d');
      g.fillStyle = 'rgba(0,0,0,.30)'; g.fillRect(0, 0, W, H);
      const lg = g.createLinearGradient(0, 0, W, 0);
      lg.addColorStop(0, 'rgba(0,0,0,.86)'); lg.addColorStop(.5, 'rgba(0,0,0,.8)'); lg.addColorStop(.66, 'rgba(0,0,0,.5)'); lg.addColorStop(.84, 'rgba(0,0,0,.08)'); lg.addColorStop(1, 'rgba(0,0,0,0)');
      g.fillStyle = lg; g.fillRect(0, 0, W, H);
      const tg = g.createLinearGradient(0, 0, 0, 56);
      tg.addColorStop(0, 'rgba(0,0,0,.9)'); tg.addColorStop(.55, 'rgba(0,0,0,.8)'); tg.addColorStop(1, 'rgba(0,0,0,0)');
      g.fillStyle = tg; g.fillRect(0, 0, W, 56);
      const bg = g.createLinearGradient(0, 184, 0, H);
      bg.addColorStop(0, 'rgba(0,0,0,0)'); bg.addColorStop(.4, 'rgba(0,0,0,.8)'); bg.addColorStop(1, 'rgba(0,0,0,.9)');
      g.fillStyle = bg; g.fillRect(0, 184, W, H - 184);
    }
    if (amount > 0) { ctx.globalAlpha = amount; ctx.drawImage(scrimCanvas, 0, 0); ctx.globalAlpha = 1; }
    if (extraDim > 0) rect(0, 0, W, H, '#000', extraDim);
  }
  function shotName(i) { return window.ARCADE.shotFor(games()[i]) === 'shot-720.png' ? 'shotL' : 'shotP'; }
  function arcMove(d) {
    const n = Math.max(0, Math.min(games().length - 1, S.arc + d)); if (n === S.arc) return;
    S.arc = n; S.shotPrev = S.shot; S.shot = shotName(n); S.shotAt = performance.now();
  }
  function arcRows(band = () => ({ dx: 0, a: 1 })) {
    ctx.save(); ctx.beginPath(); ctx.rect(ARC.L - 2, ARC.TOP, ARC.R - ARC.L + 4, ARC.BOTTOM - ARC.TOP); ctx.clip();
    const fy = ARC.TOP + ARC.FOCUS * ROW, fb = band(0);
    const g = ctx.createLinearGradient(ARC.L, 0, ARC.R, 0);
    g.addColorStop(0, 'rgba(90,26,20,.95)'); g.addColorStop(.6, 'rgba(48,14,11,.8)'); g.addColorStop(1, 'rgba(0,0,0,0)');
    ctx.globalAlpha = fb.a; ctx.fillStyle = g; ctx.fillRect(ARC.L + fb.dx, fy, ARC.R - ARC.L, ROW);
    rect(ARC.L + fb.dx, fy, 2, ROW, ARC.RED, fb.a);
    games().forEach((name, i) => {
      const y = ARC.TOP + (i - S.arcScroll + ARC.FOCUS) * ROW;
      if (y < ARC.TOP - ROW || y > ARC.BOTTOM) return;
      const b = band(1 + Math.max(0, Math.round(i - S.arcScroll + ARC.FOCUS)));
      const k = name.search(/ [(\[]/), base = k < 0 ? name : name.slice(0, k), rest = k < 0 ? '' : name.slice(k + 1);
      const x = ARC.L + 8 + b.dx, on = i === S.arc;
      // rows fade toward the top and bottom of the window
      const d = Math.abs(y - (ARC.TOP + ARC.FOCUS * ROW)) / ROW, fade = on ? 1 : Math.max(0.35, 1 - Math.max(0, d - 2) * 0.22) * (S.ask ? 0.3 : 1);
      text(base, x, y + TEXT_DY, on ? '#fff3ec' : C.cream, b.a * fade, 1);
      if (rest) text(rest, x + tw(base, 1) + 6, y + TEXT_DY, on ? '#f3b5ab' : C.muted, b.a * fade, 1);
    });
    ctx.restore();
  }
  function arcChrome(labelA = 1) {
    text('ARCADE' + (S.asec === 1 ? ' / RECENT' : S.asec === 2 ? ' / FAVOURITES' : ''), L, HEADER_RULE + 8, C.muted, labelA);
    const n = `${S.arc + 1} / ${games().length}`;
    text(n, ARC.R - tw(n, 1), HEADER_RULE + 8, C.muted, labelA, 1);
  }
  function arcFooter(a) {
    footer('A PLAY', 'B BACK', a);
    text('SELECT OVERVIEW', 210, FOOTER_RULE + 5, C.muted, a, 1);
    const hint = 'X OPTIONS   Y SEARCH';
    text(hint, R - tw(hint, 1), FOOTER_RULE + 5, C.muted, a, 1);
  }
  function arcFooterHub(a) {
    footer('A OPEN', 'B BACK', a);
    const hint = 'SELECT GAME LIST'; text(hint, R - tw(hint, 1), FOOTER_RULE + 5, C.muted, a, 1);
  }
  // Arcade content for either side of the left panel.
  function arcContent(mode, band) {
    if (mode === 'hub') drawSystem(band, ARC_SYS, S.arow || 0);
    else { arcChrome(band(0).a); arcRows(band); }
  }
  function arcSection(i) {
    const n = [999, ARC_RECENT.length, arcFavs().length][i]; if (!n) return;
    S.asec = i; S.arcList = i === 1 ? ARC_RECENT : i === 2 ? arcFavs() : null;
    S.arc = S.arcScroll = i === 0 ? 5 : Math.min(ARC.FOCUS, games().length - 1);
    S.shotPrev = S.shot; S.shot = shotName(S.arc); S.shotAt = performance.now();
    arcToggle();
  }
  // Select: the left side swaps between the hub and the list; the screenshot stays.
  function arcToggle() {
    const to = S.amode === 'hub' ? 'list' : 'hub';
    animate(340, tt => {
      drawBackdrop(); scrim(); chrome(null);
      const half = tt < 150;
      if (!half && S.amode !== to) S.amode = to;
      const band = half ? (() => { const k = 1 - inOut(tt / 150); return { dx: -Math.round(24 * (1 - k)), a: k }; })
        : (i => { const k = out(win(tt, 150 + Math.min(i, 8) * 14, 190)); return { dx: Math.round(24 * (1 - k)), a: k }; });
      arcContent(S.amode === 'hub' ? 'hub' : 'list', band);
      if (S.amode === 'hub') arcFooterHub(1); else arcFooter(1);
    }, () => { S.amode = to; });
  }
  // The CRT launcher with the new cabinet card face (demo composite).
  let arcLauncher = null;
  function arcadeLauncher() {
    if (arcLauncher) return arcLauncher;
    arcLauncher = document.createElement('canvas'); arcLauncher.width = W; arcLauncher.height = H;
    const g = arcLauncher.getContext('2d'), keep = ctx; ctx = g;
    ctx.drawImage(img.launcherArc, 0, 0);
    ctx.save(); roundRectPath(CARD_FACE.x, CARD_FACE.y, CARD_FACE.w, CARD_FACE.h, 3); ctx.clip();
    ctx.imageSmoothingEnabled = true; ctx.drawImage(img.cabCard, CARD_FACE.x, CARD_FACE.y, CARD_FACE.w, CARD_FACE.h); ctx.restore();
    ctx.imageSmoothingEnabled = false;
    text('ARCADE', 319.5 - tw('ARCADE') / 2, 134, C.cream);
    text('999 GAMES', 319.5 - tw('999 GAMES') / 2, 149, C.cream);
    ctx = keep; return arcLauncher;
  }
  function drawArcadeStatic() {
    drawBackdrop(); scrim(); chrome(null);
    if (S.amode === 'hub') { drawSystem(undefined, ARC_SYS, S.arow || 0); arcFooterHub(1); }
    else { arcChrome(); arcRows(); arcFooter(1); }
  }
  // Card zoom: the red outline zooms past the edges; inside it the selected
  // game grows from the card face to fill the TV, as if the card's art became
  // the screen. The scrim settles in after, then the rows deal in.
  const ARC_ZOOM_MS = 900;
  function arcZoomFrame(t) {
    const z = Math.exp(Math.log(ZMAX) * inOut(win(t, 0, 680)));
    const pc = inOut(win(t, 40, 640));
    ctx.globalAlpha = 1 - win(t, 110, 320); ctx.drawImage(arcadeLauncher(), 0, 0); ctx.globalAlpha = 1;
    const ww = CARD.w * z, wh = CARD.h * z, wx = CCX - ww / 2, wy = CCY - wh / 2, wr = 4 * z;
    const into = { x: lerp(CARD_FACE.x, 0, pc), y: lerp(CARD_FACE.y, 0, pc), w: lerp(CARD_FACE.w, W, pc), h: lerp(CARD_FACE.h, H, pc) };
    ctx.save(); roundRectPath(wx, wy, ww, wh, wr); ctx.clip();
    ctx.fillStyle = '#000'; ctx.fillRect(0, 0, W, H);
    drawBackdrop(1, into);
    scrim(win(t, 420, 300));
    const faceA = 1 - win(t, 30, 200);
    if (faceA > 0) {
      ctx.globalAlpha = faceA;
      ctx.drawImage(arcadeLauncher(), CARD.x, CARD.y, CARD.w, CARD.h, CCX - CARD.w * z / 2, CCY - CARD.h * z / 2, CARD.w * z, CARD.h * z);
      ctx.globalAlpha = 1;
    }
    ctx.restore();
    const outlineA = Math.max(0, 1 - 1.25 * Math.pow(win(t, 0, 680), 2));
    if (outlineA > 0) { ctx.globalAlpha = outlineA; ctx.strokeStyle = ARC.RED; ctx.lineWidth = 2 * Math.sqrt(z); roundRectPath(wx, wy, ww, wh, wr); ctx.stroke(); ctx.globalAlpha = 1; }
    chrome(null);
    footer('A OPEN', 'B BACK', 1 - win(t, 60, 120));
    const deal = i => { const k = out(win(t, 500 + Math.min(i, 10) * 22, 260)); return { dx: Math.round(24 * (1 - k)), a: k }; };
    if (S.amode === 'hub') { drawSystem(deal, ARC_SYS, S.arow || 0); arcFooterHub(win(t, 560, 200)); }
    else { arcChrome(win(t, 480, 200)); arcFooter(win(t, 560, 200)); arcRows(deal); }
  }
  function arcZoom(dir) {
    if (dir > 0) { S.amode = 'hub'; S.arow = 0; S.asec = 0; S.arcList = null; S.arc = S.arcScroll = 5; S.shot = shotName(S.arc); S.shotAt = 0; }
    if (!S.shot) { S.shot = shotName(S.arc); S.shotAt = 0; }
    animate(ARC_ZOOM_MS, t => arcZoomFrame(dir > 0 ? t : ARC_ZOOM_MS - t), () => { S.scr = dir > 0 ? 'arcade' : 'launcher'; });
  }

  // ---------- Arcade Search (240p): QWERTY left, results right ----------
  const SK = { x: 38, w: 22, gap: 3, fieldY: 56, rowY: 78, rowH: 14, rowGap: 2, resX: 308 };
  const SKEYS = ['1234567890', 'QWERTYUIOP', "ASDFGHJKL'", 'ZXCVBNM-.&'].map(r => r.split(''))
    .concat([[{ k: ' ', label: 'SPACE', span: 4 }, { k: 'DEL', label: 'DEL', span: 3 }, { k: 'CLEAR', label: 'CLEAR', span: 3 }]]);
  const SQ = { q: '', zone: 'keys', r: 1, c: 0, res: 0, results: [] };
  function skeyAt(r, c) {
    if (r < 4) return SKEYS[r][c];
    let col = 0; for (const k of SKEYS[4]) { if (c < col + k.span) return k; col += k.span; } return SKEYS[4][2];
  }
  function sfilter() {
    const q = SQ.q.trim(), all = fullGames();
    SQ.results = q ? all.filter(g => g.toUpperCase().includes(q)) : all.slice();
    SQ.res = Math.min(SQ.res, Math.max(0, SQ.results.length - 1));
  }
  function drawSearch(band = () => ({ dx: 0, a: 1 })) {
    const b0 = band(0);
    // field: typed text with a red block cursor on a red rule
    text(SQ.q, SK.x + 4 + b0.dx, SK.fieldY, C.cream, b0.a);
    if (Math.floor(performance.now() / 500) % 2 === 0) rect(SK.x + 6 + tw(SQ.q) + b0.dx, SK.fieldY + 1, 10, 11, ARC.RED, b0.a);
    rect(SK.x + b0.dx, SK.fieldY + 15, 10 * SK.w + 9 * SK.gap, 1, ARC.RED, b0.a);
    SKEYS.forEach((row, r) => {
      const b = band(1 + r), y = SK.rowY + r * (SK.rowH + SK.rowGap);
      let col = 0;
      row.forEach(k => {
        const key = typeof k === 'string' ? { k, label: k, span: 1 } : k;
        const x = SK.x + col * (SK.w + SK.gap) + b.dx, w = key.span * SK.w + (key.span - 1) * SK.gap;
        const on = SQ.zone === 'keys' && skeyAt(SQ.r, SQ.c) === k;
        rect(x, y, w, SK.rowH, on ? ARC.RED : C.rule, b.a);
        rect(x + 1, y + 1, w - 2, SK.rowH - 2, on ? '#3a1511' : '#0b0b0d', b.a);
        text(key.label, x + (w - tw(key.label)) / 2, y + 1, on ? '#fff' : C.cream, b.a);
        col += key.span;
      });
    });
    // results on the right
    const first = Math.max(0, Math.min(SQ.res - 3, SQ.results.length - 9));
    SQ.results.slice(first, first + 9).forEach((g, i) => {
      const b = band(2 + i), y = LIST_TOP + i * ROW, on = SQ.zone === 'results' && first + i === SQ.res, x = SK.resX + b.dx;
      if (on) {
        const gr = ctx.createLinearGradient(x, 0, R, 0);
        gr.addColorStop(0, '#3a1511'); gr.addColorStop(.65, '#1d0a08'); gr.addColorStop(1, 'rgba(0,0,0,0)');
        ctx.globalAlpha = b.a; ctx.fillStyle = gr; ctx.fillRect(x, y, R - SK.resX, ROW); ctx.globalAlpha = 1; rect(x, y, 2, ROW, ARC.RED, b.a);
      }
      const k = g.search(/ [(\[]/), base = k < 0 ? g : g.slice(0, k);
      ctx.save(); ctx.beginPath(); ctx.rect(SK.resX, y, R - SK.resX, ROW); ctx.clip();
      text(base, x + 8, y + TEXT_DY, on ? '#fff3ec' : C.cream, b.a, 1);
      if (k >= 0) text(g.slice(k + 1), x + 8 + tw(base, 1) + 6, y + TEXT_DY, on ? '#f3b5ab' : C.muted, b.a, 1);
      ctx.restore();
      if (!on) rect(x + 8, y + ROW - 1, R - x - 8, 1, C.rule, b.a * 0.8);
    });
  }
  function searchChrome(a = 1) {
    text('ARCADE / SEARCH', L, HEADER_RULE + 8, C.muted, a);
    const n = `${SQ.results.length} MATCH${SQ.results.length === 1 ? '' : 'ES'}`;
    text(n, R - tw(n), HEADER_RULE + 8, C.muted, a);
  }
  function searchFooter(a) {
    footer('A TYPE', 'B BACK', a);
    const hint = 'DOWN RESULTS   X OPTIONS';
    text(hint, R - tw(hint, 1), FOOTER_RULE + 5, C.muted, a, 1);
  }
  function stype(k) {
    if (k === 'DEL') SQ.q = SQ.q.slice(0, -1); else if (k === 'CLEAR') SQ.q = ''; else if (SQ.q.length < 20) SQ.q += k;
    sfilter();
  }
  function searchInput(k, A, up, down, left, right) {
    if (SQ.zone === 'keys') {
      if (down && SQ.r === 4 && SQ.results.length) { SQ.zone = 'results'; SQ.res = 0; }
      else if (up || down) SQ.r = Math.max(0, Math.min(4, SQ.r + (down ? 1 : -1)));
      else if (left || right) { let c = SQ.c; const cur = skeyAt(SQ.r, c); do { c = (c + (right ? 1 : 9)) % 10; } while (skeyAt(SQ.r, c) === cur && c !== SQ.c); SQ.c = c; }
      else if (A) { const key = skeyAt(SQ.r, SQ.c); stype(typeof key === 'string' ? key : key.k); }
      else if (/^[0-9c-z'&.\- ]$/i.test(k)) stype(k.toUpperCase());
    } else {
      if (up && SQ.res === 0) SQ.zone = 'keys'; else if (up) SQ.res--; else if (down) SQ.res = Math.min(SQ.results.length - 1, SQ.res + 1);
      if ((up || down) && SQ.zone === 'results') { const n = shotName(fullGames().indexOf(SQ.results[SQ.res])); if (n !== S.shot) { S.shotPrev = S.shot; S.shot = n; S.shotAt = performance.now(); } }
      else if (A) { S.arcList = null; S.asec = 0; S.arc = S.arcScroll = fullGames().indexOf(SQ.results[SQ.res]); S.shot = shotName(S.arc); searchPush(-1); }
    }
  }
  // Arcade <-> Search: the cabinet fades, the list pushes out, keys and
  // results deal in row by row. dir -1 plays it back to the list.
  const SEARCH_MS = 560;
  function searchFrame(t) {
    const ko = win(t, 0, 180);
    drawBackdrop(); scrim(1, 0.45 * inOut(win(t, 0, 300)));
    chrome(null);
    arcChrome(1 - ko); searchChrome(win(t, 120, 180));
    arcFooter(1 - win(t, 0, 140)); searchFooter(win(t, 200, 180));
    if (ko < 1) arcRows(() => ({ dx: Math.round(-32 * inOut(ko)), a: 1 - ko }));
    drawSearch(i => { const k = out(win(t, 120 + i * 24, 280)); return { dx: Math.round(24 * (1 - k)), a: k }; });
  }
  function searchPush(dir) {
    if (dir > 0) { SQ.zone = 'keys'; sfilter(); }
    animate(SEARCH_MS, t => searchFrame(dir > 0 ? t : SEARCH_MS - t), () => { S.scr = dir > 0 ? 'search' : 'arcade'; });
  }

  // ---------- chrome ----------
  function chrome(section, footA, footB, secAlpha = 1) {
    text('MISTER MAGIK', L, MY);
    // Demo: match the clock of whichever launcher capture is on screen.
    const clock = S.card === 'arcade' ? '13:07' : '09:15';
    text(clock, R - tw(clock), MY);
    rect(L, HEADER_RULE, R - L, 1, C.rule);
    if (section) text(section, L, HEADER_RULE + 8, C.muted, secAlpha);
    rect(L, FOOTER_RULE, R - L, 1, C.rule);
  }
  function footer(a, b, alpha = 1) {
    text(a, L, FOOTER_RULE + 5, C.cream, alpha);
    if (b) text(b, L + tw(a) + 36, FOOTER_RULE + 5, C.cream, alpha);
  }

  // ---------- list rows ----------
  function rowValue(r, focused) {
    if (r.toggle) return S.reduce ? 'ON' : 'OFF';
    if (r.stepper) return STEPS[S.step];
    return r.v;
  }
  function drawRow(r, y, focused, dx = 0, alpha = 1, dim = 1) {
    const a = alpha * dim, x = L + dx;
    if (focused) {
      const g = ctx.createLinearGradient(x, 0, R + dx, 0);
      g.addColorStop(0, C.focusA); g.addColorStop(.6, C.focusB); g.addColorStop(1, 'rgba(0,0,0,0)');
      ctx.globalAlpha = a; ctx.fillStyle = g; ctx.fillRect(x, y, R - L, ROW); ctx.globalAlpha = 1;
      rect(x, y, 2, ROW, C.violet, a);
    }
    text(r.l, x + 8, y + TEXT_DY, focused ? C.violet : r.info ? '#c9c3b3' : C.cream, a);
    let v = rowValue(r), right = R - 6 + dx;
    if (r.link) { text('>', right - 12, y + TEXT_DY, focused ? C.violet : C.muted, a); right -= 24; }
    if (r.stepper && focused) {
      text('>', right - 12, y + TEXT_DY, S.step === STEPS.length - 1 ? C.btn : C.violet, a); right -= 24;
      text(v, right - tw(v), y + TEXT_DY, C.cream, a); right -= tw(v) + 12;
      text('<', right - 12, y + TEXT_DY, S.step === 0 ? C.btn : C.violet, a);
      return;
    }
    const on = r.toggle && S.reduce;
    text(v, right - tw(v), y + TEXT_DY, on ? C.violet : focused ? C.cream : C.muted, a);
  }
  function groupRules(rows, alpha) {
    for (let i = 1; i < rows.length; i++) if (rows[i].g !== rows[i - 1].g || rows[i].gap)
      rect(L + 8, rows[i].y - GROUP_GAP / 2 - 1, R - L - 8, 1, C.rule, alpha);
  }

  // ---------- screens (static, with per-band offsets for transitions) ----------
  function bandsOf(scr) {
    if (scr === 'settings') return ROWS.length;
    if (scr === 'about') return ABOUT.length + 3;
    if (scr === 'licenses') return Math.min(LIC_PAGE, LICENSES.length - Math.floor(S.lic / LIC_PAGE) * LIC_PAGE);
    if (scr === 'license') return 1;
    return 0;
  }
  function drawScreen(scr, band = () => ({ dx: 0, a: 1 }), extra = {}) {
    const dim = S.dd || (S.ask && S.ask.anchor) ? 0.35 : 1;
    if (scr === 'settings') {
      groupRules(ROWS, band(0).a * dim);
      ROWS.forEach((r, i) => { const b = band(i); drawRow(r, r.y, i === S.sel, b.dx, b.a, i !== S.sel ? dim : 1); });
    } else if (scr === 'about') {
      groupRules(ABOUT, band(0).a);
      ABOUT.forEach((r, i) => { const b = band(i); drawRow(r, r.y, r.link, b.dx, b.a); });
      const credits = [['AN UNINTUITIVE.COM PRODUCTION', C.cream], ['CRAFTED IN FINLAND BY NIGEL BRESLAW', '#c9c3b3'],
        ['THANKS TO THE MISTER PROJECT', C.muted], ['MADE WITH SLINT', C.muted]];
      credits.forEach(([s, c], i) => { const b = band(ABOUT.length + Math.min(i, 2)); text(s, L + 8 + b.dx, 140 + i * 14, c, b.a); });
    } else if (scr === 'licenses') {
      const first = Math.floor(S.lic / LIC_PAGE) * LIC_PAGE;
      LICENSES.slice(first, first + LIC_PAGE).forEach(([n, k], i) => {
        const b = band(i), y = LIST_TOP + i * ROW;
        drawRow({ l: n, v: k, link: true }, y, first + i === S.lic, b.dx, b.a);
      });
    } else if (scr === 'license') {
      const b = band(0);
      TEXT_LINES.slice(S.textTop, S.textTop + TEXT_VIEW).forEach((line, i) =>
        text(line, L + 8 + b.dx, LIST_TOP + i * 12, '#d8d2c2', b.a, 1));
      // fade the last line into black and show the position
      const track = TEXT_VIEW * 12, hgt = Math.max(6, track * TEXT_VIEW / TEXT_LINES.length);
      rect(R + 2, LIST_TOP, 1, track, '#1c1826', b.a);
      rect(R + 1, LIST_TOP + (track - hgt) * S.textTop / Math.max(1, TEXT_LINES.length - TEXT_VIEW), 3, hgt, C.violet, b.a);
    }
  }
  const SECTION = { settings: 'SETTINGS', about: 'SETTINGS / ABOUT', licenses: 'ABOUT / LICENSES', license: 'LICENSES / MISTER MAGIK' };
  const FOOT = { settings: ['A CHANGE', 'B BACK'], about: ['A OPEN', 'B BACK'], licenses: ['A READ', 'B BACK'], license: ['B BACK', 'UP DOWN SCROLL'] };

  // ---------- drop-down and confirmation ----------
  const OPT = 12;
  function ddGeometry() {
    const r = ROWS[S.sel], items = CHOICES[r.combo];
    const h = items.reduce((a, i) => a + (i === null ? 3 : OPT), 0) + 4;
    const w = 240, x = R - w;
    let y = r.y + ROW + 1;
    if (y + h > FOOTER_RULE - 2) y = Math.max(LIST_TOP, r.y - 1 - h);
    return { x, y, w, h, items };
  }
  function drawDropdown(open) {
    const dd = S.dd; if (!dd) return;
    if (dd.confirm) return drawConfirm(open);
    const { x, y, w, h, items } = ddGeometry(), hh = Math.round(h * open);
    rect(x - 1, y - 1, w + 2, hh + 2, C.panelEdge, open);
    rect(x, y, w, hh, C.panel, 1);
    rect(x - 1, y - 1, w + 2, 1, C.violet, open);
    ctx.save(); ctx.beginPath(); ctx.rect(x, y, w, hh); ctx.clip();
    let yy = y + 2;
    items.forEach((it, i) => {
      if (it === null) { rect(x + 8, yy + 1, w - 16, 1, C.rule, open); yy += 3; return; }
      const hi = i === dd.hi;
      if (hi) {
        const g = ctx.createLinearGradient(x, 0, x + w, 0);
        g.addColorStop(0, C.focusA); g.addColorStop(.7, C.focusB); g.addColorStop(1, 'rgba(0,0,0,0)');
        ctx.fillStyle = g; ctx.fillRect(x, yy, w, OPT); rect(x, yy, 2, OPT, C.violet);
      }
      text(it, x + 8, yy, hi ? C.cream : C.muted, open);
      if (it === ROWS[S.sel].v) rect(x + w - 10, yy + 5, 4, 2, C.violet, open);
      yy += OPT;
    });
    ctx.restore();
  }
  function drawConfirm(open) {
    const c = S.dd.confirm, r = ROWS[S.sel], w = 300, h = 50, x = R - w;
    let y = r.y + ROW + 1; if (y + h > FOOTER_RULE - 2) y = r.y - 1 - h;
    rect(x - 1, y - 1, w + 2, h + 2, C.panelEdge, open); rect(x, y, w, h, C.panel);
    rect(x - 1, y - 1, w + 2, 1, C.violet, open);
    text(r.combo === 'display' ? 'KEEP THIS MODE?' : 'KEEP ORIENTATION?', x + 8, y + 4, C.cream, open);
    text(c.choice, x + 8, y + 18, C.violet, open);
    const bw = (w - 24) / 2;
    [['KEEP', 0], ['REVERT ' + c.left, 1]].forEach(([s, i]) => {
      const bx = x + 8 + i * (bw + 8), on = c.focus === i;
      rect(bx, y + 33, bw, 13, on ? C.violet : C.btn, open); rect(bx + 1, y + 34, bw - 2, 11, on ? C.btnOn : C.panel);
      text(s, bx + (bw - tw(s)) / 2, y + 34, on ? C.cream : C.muted, open);
    });
  }

  // ---------- confirmation dialogs (same component as HDMI ask.js) ----------
  // Anchored under the row that asked, or centred over a scrim when nothing on
  // screen asked. Violet in Settings, red in Arcade. Title in the 12x12 cell,
  // message in native 6x12 so it wraps to few lines.
  const ASKC = {
    violet: { panel: C.panel, edge: C.panelEdge, acc: C.violet, on: C.btnOn, btn: C.btn },
    red: { panel: '#0d0807', edge: '#6b2e27', acc: ARC.RED, on: '#2a0f0c', btn: '#4a2723' },
  };
  function askOpen(spec) {
    const a = { focus: 0, ...window.ASK.KINDS[spec.kind], ...spec, at: performance.now() };
    if (a.countdown) a.timer = setInterval(() => { if (--a.countdown <= 0) askChoose(0); }, 1000);
    S.ask = a;
  }
  function askChoose(i) { const a = S.ask; if (!a) return; clearInterval(a.timer); S.ask = null; a.onChoose && a.onChoose(i); }
  function wrap(str, cols) {
    const out = []; let line = '';
    for (const w of str.split(' ')) { if (line && (line + ' ' + w).length > cols) { out.push(line); line = w; } else line = line ? line + ' ' + w : w; }
    if (line) out.push(line); return out;
  }
  function askLayout(a) {
    const w = a.anchor ? (a.anchor.w || 300) : 360;
    const title = (a.title || '').toUpperCase(), tsx = tw(title) <= w - 16 ? 2 : 1;
    const lines = wrap(a.msg, Math.floor((w - 16) / 6));
    const h = 6 + (title ? 14 : 0) + (a.value ? 13 : 0) + lines.length * 11 + 6 + 14 + 6;
    let x, y, up = false;
    if (!a.anchor) { x = Math.round((W - w) / 2); y = Math.round((HEADER_RULE + FOOTER_RULE - h) / 2); }
    else {
      x = a.anchor.left != null ? a.anchor.left : R - w;
      y = a.anchor.y + ROW + 1;
      if (y + h > FOOTER_RULE - 2) { y = a.anchor.y - 1 - h; up = true; }
    }
    return { x, y, w, h, up, title, tsx, lines };
  }
  function drawAsk() {
    const a = S.ask; if (!a) return;
    const k = a.instant ? 1 : out(clamp01((performance.now() - a.at) / (140 * slow)));
    const g = askLayout(a), c = ASKC[a.accent || 'violet'], hh = Math.round(g.h * k);
    const top = g.up ? g.y + g.h - hh : g.y;
    if (!a.anchor) rect(0, HEADER_RULE + 1, W, FOOTER_RULE - HEADER_RULE - 1, '#000', 0.62 * k);
    rect(g.x - 1, top - 1, g.w + 2, hh + 2, c.edge, k); rect(g.x, top, g.w, hh, c.panel);
    rect(g.x - 1, g.up ? top + hh : top - 1, g.w + 2, 1, c.acc, k);
    ctx.save(); ctx.beginPath(); ctx.rect(g.x, top, g.w, hh); ctx.clip();
    let y = g.y + 6;
    if (g.title) { text(g.title, g.x + 8, y, C.cream, k, g.tsx); y += 14; }
    if (a.value) { text(a.value, g.x + 8, y, c.acc, k); y += 13; }
    g.lines.forEach(l => { text(l, g.x + 8, y, C.muted, k, 1); y += 11; });
    y += 6;
    const btns = [a.left, a.right].filter(Boolean), gap = 6, bw = (g.w - 16 - gap * (btns.length - 1)) / btns.length;
    btns.forEach((b, i) => {
      const bx = g.x + 8 + i * (bw + gap), on = a.focus === i, s = i === 0 && a.countdown ? `${b} ${a.countdown}` : b;
      rect(bx, y, bw, 14, on ? c.acc : c.btn, k); rect(bx + 1, y + 1, bw - 2, 12, on ? c.on : c.panel);
      if (on) rect(bx + 1, y + 1, 2, 12, c.acc, k);
      text(s, bx + (bw - tw(s)) / 2, y + 1, on ? C.cream : C.muted, k);
    });
    ctx.restore();
    // the dialog owns the footer while it is open
    rect(0, FOOTER_RULE + 1, W, H - FOOTER_RULE - 1, '#000');
    if (btns.length > 1) {
      footer('A CHOOSE', 'B ' + a.left);
      text('LEFT RIGHT MOVE', R - tw('LEFT RIGHT MOVE', 1), FOOTER_RULE + 5, C.muted, 1, 1);
    } else footer('A OK');
  }
  function askKey(A, B, left, right) {
    const a = S.ask, n = [a.left, a.right].filter(Boolean).length;
    if ((left || right) && n > 1) a.focus = 1 - a.focus;
    else if (A) askChoose(a.focus); else if (B) askChoose(0);
  }
  const rowAnchor = i => ({ y: ROWS[i].y });
  function askGame() {
    const g = games()[S.arc], fav = /^(1942|720|Action Fighter)/.test(g);
    askOpen({ kind: fav ? 'remove-favourite' : 'add-favourite', title: g.replace(/ [(\[].*/, ''), accent: 'red',
      anchor: { y: ARC.TOP + ARC.FOCUS * ROW, left: ARC.L, w: 320 } });
  }

  // ---------- collection browsing (240p): Consoles -> maker -> system ----------
  // Same data and design as browse.js. Browse views are the CRT launcher with
  // generic cards and a breadcrumb header; the system view is one CRT page
  // for every system, entered by the card zoom in the collection colour.
  const BR = { path: [], sel: [2, 3, 2], sys: null, row: 0 };
  const BRW = () => window.BROWSE;
  const brHere = () => BR.path[BR.path.length - 1];
  const brFocused = () => brHere().children[BR.sel[BR.path.length - 1] || 0];
  // Generic card at the CRT launcher size: 160 display x 112 raster lines.
  function crtCard(node) {
    if (node.crt) return node.crt;
    if (node.art) return crtRootCard(node);
    const B0 = BRW(), B = { mix: B0.mix, games: B0.games, ACC: B0.accOf(node), ACC_DEEP: B0.deepOf(node), GAMEPAD: B0.iconOf(node) }, cw = 160, ch = 112, c = document.createElement('canvas'); c.width = cw; c.height = ch;
    const g = c.getContext('2d'), keep = ctx; ctx = g;
    g.save(); g.scale(1, .5); roundRectPath(0, 0, cw, ch * 2, 9); g.restore(); g.save(); g.clip();
    const bg = g.createLinearGradient(0, 0, 0, ch);
    bg.addColorStop(0, B.mix('#0c161e', B.ACC, .34)); bg.addColorStop(.55, B.mix('#0c161e', B.ACC, .16)); bg.addColorStop(1, B.mix('#05080c', B.ACC, .08));
    g.fillStyle = bg; g.fillRect(0, 0, cw, ch);
    const ix = (cw - 64) / 2, iy = 22;                       // gamepad: 4x2 raster per bit (square on the TV)
    [[2, 1, B.ACC_DEEP], [0, 0, C.cream]].forEach(([ox, oy, col]) => { g.fillStyle = col;
      B.GAMEPAD.forEach((row, y) => [...row].forEach((b, x) => { if (b === '0') return;
        g.fillStyle = b === '2' && !ox ? B.mix('#05070c', B.ACC, .5) : col; g.fillRect(ix + x * 4 + ox, iy + y * 2 + oy, 4, 2); })); });
    g.restore();
    g.save(); g.scale(1, .5); roundRectPath(1, 2, cw - 2, ch * 2 - 4, 8); g.lineWidth = 2; g.strokeStyle = B.ACC; g.stroke(); g.restore();
    const name = node.name, sx = tw(name) <= cw - 8 ? 2 : 1;
    text(name, (cw - tw(name, sx)) / 2, 78, C.cream, 1, sx);
    const n = `${B.games(node)} GAMES`; text(n, (cw - tw(n, 1)) / 2, 94, C.cream, 0.85, 1);
    ctx = keep; node.crt = c; return c;
  }
  // Levels below the root: the selected card sits at the left and the rest
  // extend into the distance to the right, each smaller and dimmer. Levels
  // with five or more cards cycle forever; smaller ones stop at their ends.
  // Stepping moves every card one slot: the front card squeezes edge-on and
  // vanishes while a new card opens out from the end of the row.
  // Each card is 10% smaller than the one in front of it and overlaps it.
  const BSHRINK = .9, BSTEP = .85, BSS = [0, 1, 2, 3, 4].map(k => Math.pow(BSHRINK, k)), BDARK = [0, .4, .55, .65, .72], BK = BSS.length, BSTEP_MS = 460;
  const BSLOTS = (() => { let x = L; return BSS.map((s, k) => { const w = 160 * s, r = { cx: x + w / 2, s, d: BDARK[k], o: 1, sx: 1 }; x += w * BSTEP; return r; }); })();
  const BGONE_L = { cx: L + 74, s: .98, d: 0, o: 0, sx: 1 }, BGONE_R = { cx: BSLOTS[BK - 1].cx + 40, s: .34, d: .8, o: 0, sx: 1 };
  const bcyc = n => n >= 2;                                 // two or more cards cycle forever
  const brel = (i, s, n) => bcyc(n) ? (((i - s) % n) + n) % n : i - s;
  const bslotOf = (rel, n) => rel < 0 || (bcyc(n) && n > BK && rel === n - 1) ? BGONE_L : rel >= BK ? BGONE_R : BSLOTS[rel];
  const bmix = (a, b, k) => ({ cx: lerp(a.cx, b.cx, k), s: lerp(a.s, b.s, k), d: lerp(a.d, b.d, k), o: lerp(a.o, b.o, k), sx: lerp(a.sx, b.sx, k) });
  // A launcher card (Settings, Arcade, Consoles...): its art at the CRT card size.
  function crtRootCard(node) {
    const cw = 160, ch = 112, c = document.createElement('canvas'); c.width = cw; c.height = ch;
    const g = c.getContext('2d'), keep = ctx; ctx = g;
    g.save(); g.scale(1, .5); roundRectPath(0, 0, cw, ch * 2, 9); g.restore(); g.save(); g.clip();
    g.fillStyle = '#000'; g.fillRect(0, 0, cw, ch);
    const a = img.art && img.art[node.art]; if (a) g.drawImage(a, 0, 0, cw, ch);
    g.restore();
    g.save(); g.scale(1, .5); roundRectPath(1, 2, cw - 2, ch * 2 - 4, 8); g.lineWidth = 2; g.strokeStyle = node.colour; g.stroke(); g.restore();
    const name = node.name, sx = tw(name) <= cw - 8 ? 2 : 1;
    text(name, (cw - tw(name, sx)) / 2, 76, C.cream, 1, sx);
    const count = node.children ? BRW().games(node) : node.games;
    if (count) { const n = `${count} GAMES`; text(n, (cw - tw(n, 1)) / 2, 92, C.cream, 0.85, 1); }
    ctx = keep; node.crt = c; return c;
  }
  // The MagiK back of a card, shown while it is turned away (sx < 0).
  function crtBack(node) {
    const acc = BRW().accOf(node); crtBack.c = crtBack.c || {};
    if (crtBack.c[acc]) return crtBack.c[acc];
    const B = { mix: BRW().mix, ACC: acc }, cw = 160, ch = 112, c = document.createElement('canvas'); c.width = cw; c.height = ch;
    const g = c.getContext('2d'), keep = ctx; ctx = g;
    g.save(); g.scale(1, .5); roundRectPath(0, 0, cw, ch * 2, 9); g.restore(); g.save(); g.clip();
    g.fillStyle = B.mix('#05070c', B.ACC, .12); g.fillRect(0, 0, cw, ch);
    g.strokeStyle = B.mix('#05070c', B.ACC, .32); g.lineWidth = 1;
    for (let i = -ch * 2; i < cw + ch * 2; i += 12) { g.beginPath(); g.moveTo(i, 0); g.lineTo(i + ch * 2, ch); g.stroke(); g.beginPath(); g.moveTo(i + ch * 2, 0); g.lineTo(i, ch); g.stroke(); }
    g.save(); g.translate(cw / 2, ch / 2); g.scale(1, .5); g.rotate(Math.PI / 4); g.fillStyle = '#05070c'; g.fillRect(-26, -26, 52, 52);
    g.lineWidth = 2; g.strokeStyle = B.ACC; g.strokeRect(-26, -26, 52, 52); g.restore();
    g.restore();
    g.save(); g.scale(1, .5); roundRectPath(1, 2, cw - 2, ch * 2 - 4, 8); g.lineWidth = 2; g.strokeStyle = B.ACC; g.stroke(); g.restore();
    text('M', (cw - tw('M')) / 2, ch / 2 - 6, C.cream);
    ctx = keep; return (crtBack.c[acc] = c);
  }
  function drawBCard(node, p, alpha = 1) {
    const o = p.o * alpha; if (o <= .01 || Math.abs(p.sx) < .04) return;
    const im = p.sx < 0 ? crtBack(node) : crtCard(node), w = 160 * p.s * Math.abs(p.sx), h = 112 * p.s, x = p.cx - w / 2, y = 110 - h / 2;
    ctx.globalAlpha = o; ctx.imageSmoothingEnabled = true; ctx.drawImage(im, x, y, w, h);
    // reflection: flipped, faint, only the top of the card
    ctx.save(); ctx.globalAlpha = o * .14; ctx.translate(0, y + h * 2 + 2); ctx.scale(1, -1);
    ctx.beginPath(); ctx.rect(x, h - 22, w, 22); ctx.clip(); ctx.drawImage(im, x, 0, w, h); ctx.restore();
    ctx.imageSmoothingEnabled = false; ctx.globalAlpha = 1;
    if (p.d) rect(x, y, w, h, '#000', p.d * o);
  }
  // The root launcher row is centred: the selected card in the middle, one
  // card either side and a third fading out behind them.
  const RSLOT = [{ cx: 320, s: 1, d: 0 }, { d: 128, s: .8, dark: .4 }, { d: 230, s: .62, dark: .58 }];
  const rslot = rel => {
    const a = Math.abs(rel), side = Math.sign(rel);
    if (a === 0) return { cx: 320, s: 1, d: 0, o: 1, sx: 1 };
    if (a > 2) return { cx: 320 + side * 230, s: .5, d: .7, o: 0, sx: 1 };
    return { cx: 320 + side * RSLOT[a].d, s: RSLOT[a].s, d: RSLOT[a].dark, o: 1, sx: 1 };
  };
  const bslotAt = (rel, n, left) => left ? bslotOf(rel, n) : rslot(rel);
  const brelAt = (i, s, n, left) => left ? brel(i, s, n) : i - s;
  const bOnLeft = () => BR.path.length > 1;
  function drawCarousel(sel, cards, alpha = 1, dx = 0) {
    const n = cards.length, left = bOnLeft(), order = cards.map((c, i) => i).sort((a, b) => Math.abs(brelAt(b, sel, n, left)) - Math.abs(brelAt(a, sel, n, left)) || brelAt(b, sel, n, left) - brelAt(a, sel, n, left));
    for (const i of order) { const p = bslotAt(brelAt(i, sel, n, left), n, left); drawBCard(cards[i], { ...p, cx: p.cx + dx }, alpha); }
  }
  // Who leaves and who enters when the selection moves by d.
  function bstepRoles(n, from, d) {
    const wrap = bcyc(n), to = wrap ? (from + d + n) % n : Math.max(0, Math.min(n - 1, from + d));
    const idx = j => wrap ? ((j % n) + n) % n : (j >= 0 && j < n ? j : -1);
    const ke = Math.min(n, BK);                              // slots in use: a short list fills only the first few
    return { to, leave: idx(d > 0 ? from : from + ke - 1), enter: idx(d > 0 ? from + ke : to) };
  }
  // Going right the front card slides away and is clipped at the row's edge,
  // and a new card comes out from behind the last one, turning 180 degrees
  // from its back to its face. Going left is the same in reverse. The end card
  // is drawn first so it is always the furthest back.
  const BOUT = { cx: L - 90, s: 1, d: 0, o: 1, sx: 1 };
  function stepCards(cards, from, to, d, leave, enter, t) {
    const n = cards.length, k = inOut(t / BSTEP_MS), front = d > 0 ? leave : enter, end = d > 0 ? enter : leave;
    ctx.save(); ctx.beginPath(); ctx.rect(L, 0, W - L, H); ctx.clip();
    if (end >= 0) {
      const home = BSLOTS[Math.min(n, BK) - 1], tuck = { ...home, cx: home.cx - 8, o: 1 }, e = d > 0 ? k : 1 - k, q = bmix(tuck, home, e);
      q.sx = Math.cos(lerp(Math.PI, 0, d > 0 ? t / BSTEP_MS : 1 - t / BSTEP_MS)); q.o = d > 0 || k < 1 ? 1 : 0;
      drawBCard(cards[end], q);
    }
    const order = cards.map((c, i) => i).sort((a, b) => brel(b, to, n) - brel(a, to, n));
    for (const i of order) {
      if (i === leave || i === enter) continue;
      drawBCard(cards[i], bmix(bslotOf(brel(i, from, n), n), bslotOf(brel(i, to, n), n), k));
    }
    if (front >= 0) drawBCard(cards[front], d > 0 ? bmix(BSLOTS[0], BOUT, k) : bmix(BOUT, BSLOTS[0], k));
    ctx.restore();
  }
  function browseStep(d) {
    const lvl = BR.path.length - 1, cards = brHere().children, from = BR.sel[lvl] || 0;
    const { to, leave, enter } = bstepRoles(cards.length, from, d);
    if (to === from) return;
    animate(BSTEP_MS, t => browseStepFrame(cards, from, to, d, leave, enter, t), () => { BR.sel[lvl] = to; });
  }
  function browseStepFrame(cards, from, to, d, leave, enter, t) {
    const n0 = brHere(), B = BRW(), right = `${B.games(n0)} GAMES`;
    text(n0.kind, L, HEADER_RULE + 8, C.muted);
    text(right, R - tw(right, 1), HEADER_RULE + 8, C.muted, 1, 1);
    stepCards(cards, from, to, d, leave, enter, t);
    browseChrome(); browseFooter();
  }
  // Root row: one card per step, the selection stops at either end.
  function browseStepRoot(d) {
    const cards = brHere().children, from = BR.sel[0] || 0, to = Math.max(0, Math.min(cards.length - 1, from + d));
    if (to === from) return;
    animate(300, t => browseStepRootFrame(cards, from, to, d, t), () => { BR.sel[0] = to; });
  }
  function browseStepRootFrame(cards, from, to, d, t) {
    const n0 = brHere(), k = inOut(t / 300);
    text(n0.kind, L, HEADER_RULE + 8, C.muted); const right = '35216 GAMES'; text(right, R - tw(right, 1), HEADER_RULE + 8, C.muted, 1, 1);
    const order = cards.map((c, i) => i).sort((a, b) => Math.abs(b - to) - Math.abs(a - to));
    for (const i of order) {
      const q = bmix(rslot(i - from), rslot(i - to), k);
      q.sx = Math.cos(62 * Math.PI / 180 * Math.sin(Math.PI * k) * (i === from || i === to ? 1 : .6));
      drawBCard(cards[i], q);
    }
    browseChrome(); browseFooter();
  }

  // ---------- the card trick: level changes ----------
  // One motion, the same as the HDMI one. The chosen card turns and travels for
  // the whole trick: edge-on and halfway there at the midpoint, face-on and home
  // at the end. Until the midpoint the other cards are pulled in behind it; after
  // it the new level's cards slide out from behind it. Going back runs the same
  // motion toward the parent.
  const BT_MS = 920, BT_EDGE = BT_MS / 2;
  function trickState(dir) {
    const oldPath = BR.path.slice(), lvl = oldPath.length - 1, oldSel = BR.sel[lvl] || 0, oldCards = oldPath[lvl].children;
    const hero = oldCards[oldSel], newPath = dir > 0 ? [...oldPath, hero] : oldPath.slice(0, -1);
    const newCards = newPath[newPath.length - 1].children, newSel = dir > 0 ? 0 : (BR.sel[newPath.length - 1] || 0);
    return { dir, oldPath, newPath, oldCards, newCards, oldSel, newSel, hero, heroNew: newCards[newSel], oldLeft: oldPath.length > 1, newLeft: newPath.length > 1 };
  }
  function browseTrick(dir) {
    const st = trickState(dir);
    animate(BT_MS, t => browseTrickFrame(st, t), () => { BR.path = st.newPath; BR.sel[st.newPath.length - 1] = st.newSel; });
  }
  function browseTrickFrame(st, t) {
    const T = BT_MS, EDGE = BT_EDGE, B = BRW(), on = st.oldCards.length, nn = st.newCards.length;
    const h0 = bslotAt(0, on, st.oldLeft), h1 = bslotAt(0, nn, st.newLeft);
    const hm = inOut(t / T), heroX = lerp(h0.cx, h1.cx, hm), ang = Math.PI * hm;
    // header text and breadcrumb: out while gathering, in while dealing
    const swapped = t > T * .45, outK = inOut(win(t, 0, 260)), inK = out(win(t, T * .55, 360)), colA = swapped ? inK : 1 - outK;
    const lvl = swapped ? st.newPath : st.oldPath, node = lvl[lvl.length - 1];
    const right = node.root ? '35216 GAMES' : `${B.games(node)} GAMES`;
    text(node.kind, L, HEADER_RULE + 8, C.muted, colA); text(right, R - tw(right, 1), HEADER_RULE + 8, C.muted, colA, 1);
    // 1. the old cards are pulled in behind the chosen card as it goes
    const behind = { cx: heroX, s: .9, d: .6, o: 1, sx: 1 };
    const gather = st.oldCards.map((c, i) => i).filter(i => i !== st.oldSel)
      .sort((a, b) => Math.abs(brelAt(b, st.oldSel, on, st.oldLeft)) - Math.abs(brelAt(a, st.oldSel, on, st.oldLeft)));
    for (const i of gather) {
      const a = bslotAt(brelAt(i, st.oldSel, on, st.oldLeft), on, st.oldLeft), e = win(t, 0, EDGE), q = bmix(a, behind, inOut(e));
      q.sx = Math.cos(Math.PI / 2 * e * e); q.o = a.o * (1 - win(t, EDGE - 20, 20));
      drawBCard(st.oldCards[i], q);
    }
    // 2. the new cards slide out from behind it, turning face-on as they land
    const near = st.newCards.map((c, i) => i).filter(i => i !== st.newSel)
      .sort((a, b) => Math.abs(brelAt(a, st.newSel, nn, st.newLeft)) - Math.abs(brelAt(b, st.newSel, nn, st.newLeft)) || a - b);
    for (const [n, i] of near.map((i, n) => [n, i]).reverse()) {
      const dest = bslotAt(brelAt(i, st.newSel, nn, st.newLeft), nn, st.newLeft), k = out(win(t, EDGE + Math.min(n, 5) * 20, T - EDGE - 100));
      const q = bmix({ cx: heroX, s: .9, d: .6, o: 0, sx: 1 }, dest, k);
      q.sx = Math.sin(Math.PI / 2 * k); q.o = (t >= EDGE ? 1 : 0) * dest.o;
      drawBCard(st.newCards[i], q);
    }
    // 3. the chosen card: the old face until edge-on, then the next level's card
    drawBCard(ang < Math.PI / 2 ? st.hero : st.heroNew, { cx: heroX, s: 1 + .04 * Math.sin(Math.PI * t / T), d: 0, o: 1, sx: Math.abs(Math.cos(ang)) });
    crumbs(lvl, swapped ? .3 + .7 * inK : 1 - .7 * outK);
    const clock = '09:15'; text(clock, R - tw(clock), MY);
    rect(L, HEADER_RULE, R - L, 1, C.rule); rect(L, FOOTER_RULE, R - L, 1, C.rule); browseFooter();
  }
  function crumbs(path, alpha = 1) {
    let x = L;
    const names = path.length > 1 ? path.slice(1) : [{ name: 'MISTER MAGIK' }];
    names.forEach((n, i) => { const last = i === names.length - 1, s = last ? n.name : n.name + ' / ';
      text(s, x, MY, last ? C.cream : C.muted, alpha); x += tw(s); });
  }
  function browseContent(alpha = 1, dx = 0) {
    const n = brHere(), B = BRW(), right = n.root ? '35216 GAMES' : `${B.games(n)} GAMES`;
    text(n.kind, L + dx, HEADER_RULE + 8, C.muted, alpha);
    text(right, R - tw(right, 1) + dx, HEADER_RULE + 8, C.muted, alpha, 1);
    drawCarousel(BR.sel[BR.path.length - 1] || 0, n.children, alpha, dx);
  }
  function browseChrome(crumbA = 1) {
    crumbs(BR.path, crumbA);
    const clock = '09:15'; text(clock, R - tw(clock), MY);
    rect(L, HEADER_RULE, R - L, 1, C.rule); rect(L, FOOTER_RULE, R - L, 1, C.rule);
  }
  function browseFooter(a = 1) {
    footer('A OPEN', 'B BACK', a);
    const s = '<  BROWSE CARDS  >'; text(s, 320 - tw(s) / 2, 194, C.muted, a);
  }
  function drawBrowseStatic() { browseChrome(); browseContent(); browseFooter(); }

  // ---------- system game list (240p) ----------
  // The Arcade list's layout for any system: the selected game's screenshot
  // fills the TV behind a scrim and the list sits on the left, in the system's
  // colour. Entered by zooming out of the system page's row.
  const SL = { L: 38, R: 400, TOP: 56, BOTTOM: 200, FOCUS: 3 };
  const rgbaHex = (h, a) => `rgba(${[1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16)).join(',')},${a})`;
  function slOpenState(section, mode = 'list') {
    const s = BR.sys, kind = s.icon || 'pad', T = window.GLIST.TITLES[kind];
    const games = section === 1 ? T.slice(0, Math.max(1, s.recent)) : section === 2 ? T.slice(3, 3 + Math.max(1, s.favs)) : T;
    const at = Math.min(SL.FOCUS, games.length - 1);
    BR.sl = { sys: s, section, games, kind, acc: BRW().accOf(s), sel: at, scroll: at, mode };
    slSetShot(true);
  }
  function slSetShot(instant) {
    const sl = BR.sl, title = sl.games[sl.sel], name = 'sl:' + sl.kind + ':' + title;
    if (!img[name]) img[name] = window.GLIST.shotCanvas(title, sl.kind);
    S.shotPrev = instant ? name : S.shot; S.shot = name; S.shotAt = instant ? -1e9 : performance.now();
  }
  function slMove(d) {
    const sl = BR.sl, n = Math.max(0, Math.min(sl.games.length - 1, sl.sel + d)); if (n === sl.sel) return;
    sl.sel = n; slSetShot(false);
  }
  function slRows(band = () => ({ dx: 0, a: 1 })) {
    const sl = BR.sl, A = sl.acc;
    ctx.save(); ctx.beginPath(); ctx.rect(SL.L - 2, SL.TOP, SL.R - SL.L + 4, SL.BOTTOM - SL.TOP); ctx.clip();
    const fy = SL.TOP + SL.FOCUS * ROW, fb = band(0);
    const g = ctx.createLinearGradient(SL.L, 0, SL.R, 0);
    g.addColorStop(0, rgbaHex(A, .6)); g.addColorStop(.6, rgbaHex(A, .24)); g.addColorStop(1, 'rgba(0,0,0,0)');
    ctx.globalAlpha = fb.a; ctx.fillStyle = g; ctx.fillRect(SL.L + fb.dx, fy, SL.R - SL.L, ROW);
    rect(SL.L + fb.dx, fy, 2, ROW, A, fb.a);
    sl.games.forEach((name, i) => {
      const y = SL.TOP + (i - sl.scroll + SL.FOCUS) * ROW;
      if (y < SL.TOP - ROW || y > SL.BOTTOM) return;
      const b = band(1 + Math.max(0, Math.round(i - sl.scroll + SL.FOCUS)));
      const k = name.search(/ [(\[:]/), base = k < 0 ? name : name.slice(0, k), rest = k < 0 ? '' : name.slice(k + 1);
      const x = SL.L + 8 + b.dx, on = i === sl.sel;
      const d = Math.abs(y - fy) / ROW, fade = on ? 1 : Math.max(0.35, 1 - Math.max(0, d - 2) * 0.22);
      text(base.toUpperCase(), x, y + TEXT_DY, on ? '#ffffff' : C.cream, b.a * fade, 1);
      if (rest) text(rest.toUpperCase(), x + tw(base, 1) + 6, y + TEXT_DY, on ? '#e9e3d0' : C.muted, b.a * fade, 1);
    });
    ctx.restore();
  }
  function slChrome(labelA = 1) {
    const sl = BR.sl, label = sl.sys.name + (sl.section === 1 ? ' / RECENT' : sl.section === 2 ? ' / FAVOURITES' : '');
    text(label, L, HEADER_RULE + 8, C.muted, labelA);
    const n = `${sl.sel + 1} / ${sl.games.length}`;
    text(n, SL.R - tw(n, 1), HEADER_RULE + 8, C.muted, labelA, 1);
  }
  function slFooter(a) {
    footer('A PLAY', 'B BACK', a);
    const hint = 'UP DOWN BROWSE'; text(hint, R - tw(hint, 1), FOOTER_RULE + 5, C.muted, a, 1);
  }
  function drawSysListStatic() { drawBackdrop(); scrim(); chrome(null); slChrome(); slRows(); slFooter(1); }
  // The page has two sides for its left panel. Select swaps them; the
  // screenshot stays. It lands on the hub (identity and the Games / Recent /
  // Favourites rows).
  function slContent(mode, band) {
    if (mode === 'hub') drawSystem(band, BR.sl.sys, BR.row);
    else { slChrome(band(0).a); slRows(band); }
  }
  function slFootMode(mode, a) {
    footer(mode === 'hub' ? 'A OPEN' : 'A PLAY', 'B BACK', a);
    const hint = mode === 'hub' ? 'SELECT GAME LIST' : 'SELECT OVERVIEW'; text(hint, R - tw(hint, 1), FOOTER_RULE + 5, C.muted, a, 1);
  }
  function drawSysPageStatic() { drawBackdrop(); scrim(); chrome(null); slContent(BR.sl.mode, () => ({ dx: 0, a: 1 })); slFootMode(BR.sl.mode, 1); }
  function slToggle() {
    const sl = BR.sl, to = sl.mode === 'hub' ? 'list' : 'hub';
    animate(340, tt => {
      drawBackdrop(); scrim(); chrome(null);
      const half = tt < 150;
      if (!half && sl.mode !== to) sl.mode = to;
      const band = half ? (() => { const k = 1 - inOut(tt / 150); return { dx: -Math.round(24 * (1 - k)), a: k }; })
        : (i => { const k = out(win(tt, 150 + Math.min(i, 8) * 14, 190)); return { dx: Math.round(24 * (1 - k)), a: k }; });
      slContent(sl.mode, band); slFootMode(sl.mode, 1);
    }, () => { sl.mode = to; });
  }
  // A on a hub row: that section's list.
  function slOpen(i) {
    const s = BR.sl.sys; if (![s.games, s.recent, s.favs][i]) return;
    slOpenState(i, 'hub'); slToggle();
  }
  // The chosen card zooms out to the whole screen: the screenshot grows with
  // it, the level fades and the hub deals in. Back plays it in reverse.
  function slPageZoomFrame(t) {
    const src = BCARD, p = inOut(win(t, 0, 680)), sl = BR.sl, A = sl.acc;
    if (BR.snapFor !== BR.sys) {
      BR.snap = BR.snap || document.createElement('canvas'); BR.snap.width = W; BR.snap.height = H;
      const keep = ctx; ctx = BR.snap.getContext('2d'); ctx.fillStyle = '#000'; ctx.fillRect(0, 0, W, H);
      browseContent(); browseChrome(); browseFooter(); ctx = keep; BR.snapFor = BR.sys;
    }
    ctx.globalAlpha = 1 - win(t, 100, 260); ctx.drawImage(BR.snap, 0, 0); ctx.globalAlpha = 1;
    const w_ = { x: lerp(src.x, 0, p), y: lerp(src.y, 0, p), w: lerp(src.w, W, p), h: lerp(src.h, H, p) }, wr = lerp(4, 0, p);
    ctx.save(); roundRectPath(w_.x, w_.y, w_.w, w_.h, wr); ctx.clip();
    ctx.fillStyle = '#000'; ctx.fillRect(0, 0, W, H);
    drawBackdrop(1, w_);
    scrim(win(t, 420, 300));
    const faceA = 1 - win(t, 30, 200);
    if (faceA > 0) { ctx.globalAlpha = faceA; ctx.imageSmoothingEnabled = true; ctx.drawImage(crtCard(sl.sys), w_.x, w_.y, w_.w, w_.h); ctx.imageSmoothingEnabled = false; ctx.globalAlpha = 1; }
    ctx.restore();
    const outlineA = Math.max(0, 1 - 1.25 * p * p);
    if (outlineA > 0) { ctx.globalAlpha = outlineA; ctx.strokeStyle = A; ctx.lineWidth = 2; roundRectPath(w_.x, w_.y, w_.w, w_.h, wr); ctx.stroke(); ctx.globalAlpha = 1; }
    text('MISTER MAGIK', L, MY, C.cream, win(t, 260, 180)); text(S.card === 'arcade' ? '13:07' : '09:15', R - tw('09:15'), MY);
    rect(L, HEADER_RULE, R - L, 1, C.rule); rect(L, FOOTER_RULE, R - L, 1, C.rule);
    footer('A OPEN', 'B BACK', 1 - win(t, 60, 120));
    slContent(sl.mode, i => { const k = out(win(t, 500 + Math.min(i, 10) * 22, 260)); return { dx: Math.round(24 * (1 - k)), a: k }; });
    slFootMode(sl.mode, win(t, 560, 200));
  }
  function slPageZoom(dir) {
    if (dir > 0) { slOpenState(0, 'hub'); BR.row = 0; BR.snapFor = null; }
    animate(SL_ZOOM_MS, tt => slPageZoomFrame(dir > 0 ? tt : SL_ZOOM_MS - tt), () => { S.scr = dir > 0 ? 'syslist' : 'browse'; });
  }
  // The system page's focused row zooms out to the whole TV: the screenshot
  // grows with it, the page fades and the list deals in.
  const SL_ZOOM_MS = 900;
  const slRowRect = () => ({ x: L, y: LIST_TOP + BR.row * ROW, w: R - L, h: ROW });
  function slZoomFrame(t) {
    const src = slRowRect(), p = inOut(win(t, 0, 680)), pageA = 1 - win(t, 40, 260), A = BR.sl.acc;
    if (pageA > 0) {
      drawIcon({ ...SYS_ICON, a: SYS_ICON.a * pageA });
      drawSystem(() => ({ dx: 0, a: pageA }));
      footer('A OPEN', 'B BACK', pageA);
    }
    const win_ = { x: lerp(src.x, 0, p), y: lerp(src.y, 0, p), w: lerp(src.w, W, p), h: lerp(src.h, H, p) }, wr = lerp(3, 0, p);
    ctx.save(); roundRectPath(win_.x, win_.y, win_.w, win_.h, wr); ctx.clip();
    ctx.fillStyle = '#000'; ctx.fillRect(0, 0, W, H);
    drawBackdrop(1, win_);
    scrim(win(t, 420, 300));
    ctx.restore();
    const outlineA = Math.max(0, 1 - 1.25 * p * p);
    if (outlineA > 0) { ctx.globalAlpha = outlineA; ctx.strokeStyle = A; ctx.lineWidth = 2; roundRectPath(win_.x, win_.y, win_.w, win_.h, wr); ctx.stroke(); ctx.globalAlpha = 1; }
    chrome(null);
    slChrome(win(t, 480, 200)); slFooter(win(t, 560, 200));
    slRows(i => { const k = out(win(t, 500 + Math.min(i, 10) * 22, 260)); return { dx: Math.round(24 * (1 - k)), a: k }; });
  }
  function slZoom(dir) {
    if (dir > 0) slOpenState(BR.row);
    animate(SL_ZOOM_MS, t => slZoomFrame(dir > 0 ? t : SL_ZOOM_MS - t), () => { S.scr = dir > 0 ? 'syslist' : 'system'; });
  }

  // System view: identical layout for every system.
  function sysRows(s) {
    return [
      { l: 'GAMES', v: `${s.games} TITLES`, link: true, g: 0 }, { l: 'RECENT', v: `${s.recent} PLAYED`, link: true, g: 0 },
      { l: 'FAVOURITES', v: `${s.favs} SAVED`, link: true, g: 0 },
      { l: 'MEDIA', v: s.media, g: 1 }, { l: 'CONTROLLER PORTS', v: String(s.ports), g: 1 },
      { l: 'MISTER CORE', v: s.name.replace(/ /g, '').slice(0, 8), g: 1 }];
  }
  const SYS_ICON = { x: 404, y: 100, sx: 12, sy: 6, a: .11 };  // the card's gamepad, grown into a watermark
  function drawIcon(p) {
    const B = BRW(); if (p.a <= 0) return;
    ctx.globalAlpha = p.a; ctx.fillStyle = B.accOf(BR.sys);
    B.iconOf(BR.sys).forEach((row, y) => [...row].forEach((b, x) => { if (b === '0') return;
      ctx.globalAlpha = b === '2' ? p.a * .45 : p.a; ctx.fillRect(p.x + x * p.sx, p.y + y * p.sy, p.sx, p.sy); }));
    ctx.globalAlpha = 1;
  }
  function drawSystem(band = () => ({ dx: 0, a: 1 }), sysArg = null, rowArg = null) {
    const s = sysArg || BR.sys, B0 = BRW(), acc = s.acc || B0.ACC, B = { mix: B0.mix, ACC: acc }, b0 = band(0), focusRow = rowArg === null ? BR.row : rowArg;
    text(s.full.length > 26 ? s.name : s.full, L + b0.dx, HEADER_RULE + 8, C.muted, b0.a);
    const meta = `${s.year}  ${s.gen}  ${s.maker}`; text(meta, R - tw(meta, 1) + b0.dx, HEADER_RULE + 8, C.muted, b0.a, 1);
    const rows = sysRows(s);
    rows.forEach((r, i) => {
      const b = band(1 + i), y = LIST_TOP + i * ROW + r.g * GROUP_GAP, x = L + b.dx, on = i === focusRow;
      if (i && r.g !== rows[i - 1].g) rect(L + 8, y - GROUP_GAP / 2 - 1, R - L - 8, 1, C.rule, b.a);
      if (on) {
        const g = ctx.createLinearGradient(x, 0, R + b.dx, 0);
        g.addColorStop(0, B.mix('#000000', B.ACC, .26)); g.addColorStop(.6, B.mix('#000000', B.ACC, .1)); g.addColorStop(1, 'rgba(0,0,0,0)');
        ctx.globalAlpha = b.a; ctx.fillStyle = g; ctx.fillRect(x, y, R - L, ROW); ctx.globalAlpha = 1; rect(x, y, 2, ROW, B.ACC, b.a);
      }
      text(r.l, x + 8, y + TEXT_DY, on ? B.ACC : r.link ? C.cream : '#c9c3b3', b.a);
      let right = R - 6 + b.dx;
      if (r.link) { text('>', right - 12, y + TEXT_DY, on ? B.ACC : C.muted, b.a); right -= 24; }
      text(r.v, right - tw(r.v), y + TEXT_DY, on ? C.cream : C.muted, b.a);
    });
  }
  function sysChrome() {
    text('MISTER MAGIK', L, MY); text('09:15', R - tw('09:15'), MY);
    rect(L, HEADER_RULE, R - L, 1, C.rule); rect(L, FOOTER_RULE, R - L, 1, C.rule);
  }
  function drawSystemStatic() {
    drawIcon(SYS_ICON); sysChrome(); drawSystem();
    footer('A OPEN', 'B BACK'); text('UP DOWN MOVE', R - tw('UP DOWN MOVE', 1), FOOTER_RULE + 5, C.muted, 1, 1);
  }
  // Card zoom into the system view (mirrors Settings/Arcade): the outline
  // zooms past the edges, the card's gamepad grows into the watermark.
  const SYS_MS = 900;
  const BCARD = { x: BSLOTS[0].cx - 80, y: 54, w: 160, h: 112 };
  function sysZoomFrame(t) {
    const B = BRW(), z = Math.exp(Math.log(ZMAX) * inOut(win(t, 0, 680))), pc = inOut(win(t, 60, 620));
    const cx = BCARD.x + 80, cy = BCARD.y + 56;
    if (BR.snapFor !== BR.sys) {                              // the browse view, captured once per zoom
      BR.snap = BR.snap || document.createElement('canvas'); BR.snap.width = W; BR.snap.height = H;
      const keep = ctx; ctx = BR.snap.getContext('2d'); ctx.fillStyle = '#000'; ctx.fillRect(0, 0, W, H);
      browseContent(); browseChrome(); browseFooter(); ctx = keep; BR.snapFor = BR.sys;
    }
    ctx.globalAlpha = 1 - win(t, 100, 260); ctx.drawImage(BR.snap, 0, 0); ctx.globalAlpha = 1;
    const ww = BCARD.w * z, wh = BCARD.h * z, wx = cx - ww / 2, wy = cy - wh / 2;
    ctx.save(); ctx.scale(1, .5); roundRectPath(wx, wy * 2, ww, wh * 2, 9 * z); ctx.restore(); ctx.save(); ctx.clip();
    ctx.fillStyle = '#000'; ctx.fillRect(0, 0, W, H);
    const faceA = 1 - win(t, 40, 180);
    if (faceA > 0) { ctx.globalAlpha = faceA; ctx.imageSmoothingEnabled = true; ctx.drawImage(crtCard(BR.sys), cx - ww / 2, cy - wh / 2, ww, wh); ctx.imageSmoothingEnabled = false; ctx.globalAlpha = 1; }
    const start = { x: cx - 32 * z, y: cy - 34 * z, sx: 4 * z, sy: 2 * z };
    drawIcon({ x: lerp(start.x, SYS_ICON.x, pc), y: lerp(start.y, SYS_ICON.y, pc), sx: lerp(start.sx, SYS_ICON.sx, pc), sy: lerp(start.sy, SYS_ICON.sy, pc),
      a: lerp(.9, SYS_ICON.a, win(t, 140, 300)) * win(t, 60, 100) });
    ctx.restore();
    const outlineA = Math.max(0, 1 - 1.25 * Math.pow(win(t, 0, 680), 2));
    if (outlineA > 0) { ctx.globalAlpha = outlineA; ctx.strokeStyle = B.ACC; ctx.lineWidth = 2 * Math.sqrt(z);
      ctx.save(); ctx.scale(1, .5); roundRectPath(wx, wy * 2, ww, wh * 2, 9 * z); ctx.restore(); ctx.stroke(); ctx.globalAlpha = 1; }
    const ca = win(t, 300, 200);
    if (ca > 0) { text('MISTER MAGIK', L, MY, C.cream, ca); text('09:15', R - tw('09:15'), MY); rect(L, HEADER_RULE, R - L, 1, C.rule); rect(L, FOOTER_RULE, R - L, 1, C.rule); }
    footer('A OPEN', 'B BACK', 1);
    drawSystem(i => { const k = out(win(t, 320 + Math.min(i, 10) * 24, 260)); return { dx: Math.round(24 * (1 - k)), a: k }; });
  }
  function sysZoom(dir) {
    animate(SYS_MS, t => sysZoomFrame(dir > 0 ? t : SYS_MS - t), () => { S.scr = dir > 0 ? 'system' : 'browse'; });
  }
  function browseOpen(where) {
    const B = BRW(); BR.sel = [2, 3, 2];
    BR.path = where === 'launcher' ? [B.ROOT] : [B.ROOT, B.TREE]; if (where === 'nintendo') BR.path.push(B.TREE.children[3]);
    S.scr = 'browse';
  }
  function browseKey(A, B, up, down, left, right, k) {
    if (S.scr === 'syslist') {
      if (k === 'Tab') slToggle();
      else if (BR.sl.mode === 'hub') {
        if (up || down) BR.row = Math.max(0, Math.min(2, BR.row + (down ? 1 : -1)));
        else if (A) slOpen(BR.row);
        else if (B) slPageZoom(-1);
      } else if (up || down) slMove(down ? 1 : -1); else if (left || right) slMove(right ? 9 : -9); else if (B) slPageZoom(-1);
      return;
    }
    if (S.scr === 'system') {
      if (up || down) BR.row = Math.max(0, Math.min(2, BR.row + (down ? 1 : -1)));
      else if (A && BR.row < 3 && sysRows(BR.sys)[BR.row].v.split(' ')[0] !== '0') slZoom(1);
      else if (B) sysZoom(-1);
      return;
    }
    const d = BR.path.length - 1, n = brHere().children.length;
    if (left || right) { if (BR.path.length > 1) browseStep(right ? 1 : -1); else browseStepRoot(right ? 1 : -1); }
    else if (A) {
      const f = brFocused();
      if (BR.path.length === 1) {
        if (f.children) browseTrick(1);
        else if (f.name === 'SETTINGS') { S.scr = 'launcher'; S.card = 'settings'; zoom(1); }
        else if (f.name === 'ARCADE') { S.scr = 'launcher'; S.card = 'arcade'; arcZoom(1); }
      }
      else if (f.children && f.children.length > 1) browseTrick(1);
      else { BR.sys = f.children ? f.children[0] : f; BR.row = 0; slPageZoom(1); }
    } else if (B) {
      if (BR.path.length > 1) browseTrick(-1);
      else S.scr = 'launcher';
    }
  }

  // ---------- frame ----------
  // Every frame is drawn from scratch, as on the device.
  function frame(now) {
    const A = S.anim; let t = 0;
    if (A) { t = (now - A.start) / slow; if (t >= A.dur) {
        A.done && A.done(); S.anim = null;
        if (S.scr === 'launcher' && !S.pinLauncher) { browseOpen('launcher'); BR.sel[0] = S.card === 'arcade' ? 1 : 0; }   // back to the launcher row
      } }
    const dt = Math.min(50, now - (frame.last || now)); frame.last = now;
    S.arcScroll += (S.arc - S.arcScroll) * (1 - Math.exp(-dt / (45 * slow)));
    if (Math.abs(S.arc - S.arcScroll) < 0.01) S.arcScroll = S.arc;
    if (BR.sl) { BR.sl.scroll += (BR.sl.sel - BR.sl.scroll) * (1 - Math.exp(-dt / (45 * slow))); if (Math.abs(BR.sl.sel - BR.sl.scroll) < 0.01) BR.sl.scroll = BR.sl.sel; }
    ctx.fillStyle = '#000'; ctx.fillRect(0, 0, W, H);
    ctx.imageSmoothingEnabled = false;
    if (S.anim && S.anim.draw) S.anim.draw(Math.min(t, S.anim.dur));
    else drawStatic();
    requestAnimationFrame(frame);
  }
  function drawStatic() {
    if (S.scr === 'launcher') { ctx.drawImage(S.card === 'arcade' ? arcadeLauncher() : img.launcher, 0, 0); drawAsk(); return; }
    if (S.scr === 'arcade') { drawArcadeStatic(); drawAsk(); return; }
    if (S.scr === 'browse') return drawBrowseStatic();
    if (S.scr === 'system') return drawSystemStatic();
    if (S.scr === 'syslist') return drawSysPageStatic();
    if (S.scr === 'search') { drawBackdrop(); scrim(1, 0.45); chrome(null); searchChrome(); drawSearch(); searchFooter(1); return; }
    drawCog({ ...COG_REST, a: S.dd || S.ask ? COG_REST.a * 0.6 : COG_REST.a });
    chrome(SECTION[S.scr]);
    drawScreen(S.scr);
    if (S.dd) { footer('A SELECT', 'B CANCEL'); drawDropdown(1); }
    else footer(...FOOT[S.scr]);
    drawAsk();
  }

  // ---------- transitions ----------
  function animate(dur, draw, done) { S.anim = { start: performance.now(), dur, draw, done }; }

  // Card zoom: outline zooms past the edges, the cog grows out of the card into
  // its watermark, the launcher fades and the rows deal in. dir -1 mirrors it.
  const ZOOM_MS = 800;
  function zoomFrame(t) {
    const z = Math.exp(Math.log(ZMAX) * inOut(win(t, 0, 600)));
    const pc = inOut(win(t, 60, 620));
    const launcherA = 1 - win(t, 100, 300), faceA = 1 - win(t, 40, 160);
    const outlineA = Math.max(0, 1 - 1.25 * Math.pow(win(t, 0, 600), 2));
    // launcher, then the window: black + cog + fading card face
    ctx.globalAlpha = launcherA; ctx.drawImage(img.launcher, 0, 0); ctx.globalAlpha = 1;
    const ww = CARD.w * z, wh = CARD.h * z, wx = CCX - ww / 2, wy = CCY - wh / 2, wr = 4 * z;
    ctx.save(); roundRectPath(wx, wy, ww, wh, wr); ctx.clip();
    ctx.fillStyle = '#000'; ctx.fillRect(0, 0, W, H);
    drawCog({ x: lerp(COG_START.x, COG_REST.x, pc), y: lerp(COG_START.y, COG_REST.y, pc),
      sx: lerp(COG_START.sx, COG_REST.sx, pc), sy: lerp(COG_START.sy, COG_REST.sy, pc),
      a: lerp(1, COG_REST.a, win(t, 380, 320)) });
    if (faceA > 0) {
      ctx.globalAlpha = faceA;
      ctx.drawImage(img.launcher, CARD.x, CARD.y, CARD.w, CARD.h, CCX - CARD.w * z / 2, CCY - CARD.h * z / 2, CARD.w * z, CARD.h * z);
      ctx.globalAlpha = 1;
    }
    ctx.restore();
    if (outlineA > 0) {
      ctx.globalAlpha = outlineA; ctx.strokeStyle = C.violet; ctx.lineWidth = 2 * Math.sqrt(z);
      roundRectPath(wx, wy, ww, wh, wr); ctx.stroke(); ctx.globalAlpha = 1;
    }
    // chrome: header shared; section label and footer crossfade in
    chrome(null);
    text('SETTINGS', L, HEADER_RULE + 8, C.muted, win(t, 440, 200));
    footer('A OPEN', 'B BACK', 1 - win(t, 60, 120));
    footer(...FOOT.settings, win(t, 520, 200));
    drawScreen('settings', i => { const k = out(win(t, 440 + i * 25, 280)); return { dx: Math.round(24 * (1 - k)), a: k }; });
  }
  function zoom(dir) {
    const start = dir > 0 ? 0 : ZOOM_MS;
    if (S.reduce) { // simple crossfade
      animate(260, t => { const k = dir > 0 ? t / 260 : 1 - t / 260;
        ctx.globalAlpha = 1 - k; ctx.drawImage(img.launcher, 0, 0); ctx.globalAlpha = k; drawStatic2('settings'); ctx.globalAlpha = 1; },
        () => { S.scr = dir > 0 ? 'settings' : 'launcher'; });
      return;
    }
    animate(ZOOM_MS, t => zoomFrame(dir > 0 ? t : ZOOM_MS - t), () => { S.scr = dir > 0 ? 'settings' : 'launcher'; });
  }
  function drawStatic2(scr) { const keep = S.scr; S.scr = scr; drawStatic(); S.scr = keep; }

  // Page push inside Settings: the cog watermark and chrome stay; the outgoing
  // page slides and fades, the section label crossfades, rows deal in.
  const PUSH_MS = 520, OUT_MS = 180, IN_AT = 110, IN_MS = 280, STAGGER = 25, TRAVEL = 24;
  function push(to, dir) {
    const from = S.scr;
    if (to === 'license') S.textTop = 0;
    animate(PUSH_MS, t => pushFrame(from, to, dir, t), () => { S.scr = to; });
  }
  function pushFrame(from, to, dir, t) {
    {
      drawCog(COG_REST);
      chrome(null);
      const ko = win(t, 0, OUT_MS);
      text(SECTION[from], L, HEADER_RULE + 8, C.muted, 1 - ko);
      footer(...FOOT[from], 1 - win(t, 0, 140));
      if (ko < 1) drawScreen(from, () => ({ dx: Math.round(-32 * dir * inOut(ko)), a: 1 - ko }));
      text(SECTION[to], L, HEADER_RULE + 8, C.muted, win(t, IN_AT, 180));
      footer(...FOOT[to], win(t, IN_AT + 80, 180));
      drawScreen(to, i => { const k = out(win(t, IN_AT + i * STAGGER, IN_MS)); return { dx: Math.round(TRAVEL * dir * (1 - k)), a: k }; });
    }
  }

  // Drop-down unfolds from its row; the confirmation replaces it in place.
  function ddOpen() {
    const r = ROWS[S.sel], items = CHOICES[r.combo], cur = items.indexOf(r.v);
    S.dd = { hi: cur < 0 ? 0 : cur, confirm: null };
    animate(140, t => { drawBase(); drawDropdown(out(t / 140)); });
  }
  function drawBase() {
    drawCog({ ...COG_REST, a: COG_REST.a * 0.6 }); chrome(SECTION.settings); drawScreen('settings'); footer('A SELECT', 'B CANCEL');
  }
  function ddClose() {
    if (!S.dd) return; clearInterval(S.dd.confirm?.timer);
    const keep = S.dd;
    animate(100, t => { S.dd = keep; drawBase(); drawDropdown(1 - t / 100); S.dd = null; }, () => { S.dd = null; });
    S.dd = null;
  }
  function ddMove(d) {
    const items = CHOICES[ROWS[S.sel].combo]; let i = S.dd.hi;
    do { i += d; } while (i >= 0 && i < items.length && items[i] === null);
    if (i >= 0 && i < items.length) S.dd.hi = i;
  }
  function ddSelect() {
    const r = ROWS[S.sel], choice = CHOICES[r.combo][S.dd.hi];
    if (choice === r.v) return ddClose();
    const display = r.combo === 'display';
    ddClose();
    askOpen({ title: display ? 'Keep this resolution?' : 'Keep orientation?', value: choice,
      msg: display ? 'It reverts on its own if you cannot see this.' : 'Is the launcher upright on the rotated monitor?',
      left: 'REVERT', right: 'KEEP', countdown: display ? 15 : 20, anchor: rowAnchor(S.sel),
      onChoose: i => { if (i === 1) r.v = choice; } });
  }

  // ---------- input ----------
  function key(k) {
    if (S.anim) return;
    const A = k === 'Enter' || k === 'a', B = k === 'Escape' || k === 'b' || k === 'Backspace';
    const up = k === 'ArrowUp', down = k === 'ArrowDown', left = k === 'ArrowLeft', right = k === 'ArrowRight';
    if (S.ask) return askKey(A, B, left, right);
    if (S.scr === 'browse' || S.scr === 'system' || S.scr === 'syslist') return browseKey(A, B, up, down, left, right, k);
    if (S.scr === 'launcher') {
      if (left || right) S.card = S.card === 'arcade' ? 'settings' : 'arcade';
      else if (A) S.card === 'arcade' ? arcZoom(1) : zoom(1);
      return;
    }
    if (S.scr === 'arcade') {
      if (k === 'Tab') { arcToggle(); return; }
      if (S.amode === 'hub') {
        if (up || down) S.arow = Math.max(0, Math.min(2, (S.arow || 0) + (down ? 1 : -1)));
        else if (A) arcSection(S.arow || 0);
        else if (B) arcZoom(-1);
        return;
      }
      if (up) arcMove(-1); else if (down) arcMove(1); else if (left) arcMove(-9); else if (right) arcMove(9);
      else if (k === 'y' || k === 'Y') searchPush(1);
      else if (k === 'x' || k === 'X') askGame();
      else if (B) arcZoom(-1);
      return;
    }
    if (S.scr === 'search') { if (k === 'Escape' || k === 'b') searchPush(-1); else if (k === 'Backspace') stype('DEL'); else searchInput(k, A, up, down, left, right); return; }
    if (S.dd) {
      const c = S.dd.confirm;
      if (c) {
        if (left || right) c.focus = 1 - c.focus;
        else if (A) { if (c.focus === 0) ROWS[S.sel].v = c.choice; ddClose(); }
        else if (B) ddClose();
        return;
      }
      if (up) ddMove(-1); else if (down) ddMove(1); else if (A) ddSelect(); else if (B) ddClose();
      return;
    }
    if (S.scr === 'settings') {
      const r = ROWS[S.sel];
      if (up) S.sel = Math.max(0, S.sel - 1); else if (down) S.sel = Math.min(ROWS.length - 1, S.sel + 1);
      else if (r.stepper && (left || right)) S.step = Math.max(0, Math.min(STEPS.length - 1, S.step + (right ? 1 : -1)));
      else if (A) {
        if (r.combo) ddOpen(); else if (r.toggle) S.reduce = !S.reduce;
        else if (r.stepper) S.step = (S.step + 1) % STEPS.length; else if (r.l === 'ABOUT') push('about', 1);
        else if (r.l === 'EXIT TO MISTER') askOpen({ kind: 'exit-to-mister', anchor: rowAnchor(S.sel) });
        else if (r.l === 'REFRESH DATABASE') askOpen({ kind: 'refresh-database', anchor: rowAnchor(S.sel) });
      } else if (B) zoom(-1);
      return;
    }
    if (S.scr === 'about') { if (A) push('licenses', 1); else if (B) push('settings', -1); return; }
    if (S.scr === 'licenses') {
      if (up) S.lic = Math.max(0, S.lic - 1); else if (down) S.lic = Math.min(LICENSES.length - 1, S.lic + 1);
      else if (left || right) S.lic = Math.max(0, Math.min(LICENSES.length - 1, S.lic + (right ? LIC_PAGE : -LIC_PAGE)));
      else if (A) push('license', 1); else if (B) push('about', -1);
      return;
    }
    if (S.scr === 'license') {
      const max = TEXT_LINES.length - TEXT_VIEW;
      if (up) S.textTop = Math.max(0, S.textTop - 1); else if (down) S.textTop = Math.min(max, S.textTop + 1);
      else if (left) S.textTop = Math.max(0, S.textTop - TEXT_VIEW); else if (right) S.textTop = Math.min(max, S.textTop + TEXT_VIEW);
      else if (A || B) push('licenses', -1);
    }
  }

  async function init(el, textLines) {
    canvas = el; ctx = canvas.getContext('2d');
    TEXT_LINES = textLines;
    const load = src => new Promise(r => { const i = new Image(); i.onload = () => r(i); i.src = src; });
    [img.launcher, img.cog, img.launcherArc, img.cabinet, img.cabCard, img.shotL, img.shotP] = await Promise.all([
      load('crt-launcher.png'), load('cog-backdrop.png'), load('crt-launcher-arcade.png'), load('arcade-cabinet.png'),
      load('arcade-card.png'), load('shot-720.png'), load('shot-actionfighter.png'), loadFont()]);
    img.art = {};
    await Promise.all(BRW().ROOT.children.map(n => load(n.art).then(i => { img.art[n.art] = i; })));
    browseOpen('launcher'); BR.sel[0] = 0;                 // the front page is the launcher row, on Settings
    ready = true; requestAnimationFrame(frame);
  }
  // Jump to a settled state for review: settings, dd-display, confirm, about, licenses, license.
  function state(name) {
    S.anim = null; S.dd = null;
    S.pinLauncher = ['launcher', 'arclauncher', 'ask-library', 'ask-failed'].includes(name);
    S.scr = ['about', 'licenses', 'license'].includes(name) ? name : name === 'launcher' ? 'launcher' : 'settings';
    if (name === 'dd-display' || name === 'confirm') { S.sel = 0; ddOpen(); S.anim = null; S.dd.hi = 8; if (name === 'confirm') ddSelect(); }
    if (name === 'dd-orientation') { S.sel = 1; ddOpen(); S.anim = null; }
    if (name === 'screensaver') S.sel = 3;
    if (name === 'zoom') { S.scr = 'launcher'; zoom(1); }
    if (name === 'consoles' || name === 'nintendo') browseOpen(name);
    if (name === 'browse-root') browseOpen('launcher');
    if (name.startsWith('pgat') || name.startsWith('pg-')) {   // pg-<h|l>[-idx] is the page, pgat-<ms>-<h|l>[-idx] a zoom frame; idx picks a collection (3 Computers, 4 Handhelds)
      const parts = name.split('-'), zf = name.startsWith('pgat'), ms = zf ? +parts[1] : 0, m = parts[zf ? 2 : 1] === 'l' ? 'list' : 'hub', idx = parts[zf ? 3 : 2];
      if (idx != null) { browseOpen('launcher'); BR.sel[0] = +idx; BR.path.push(BRW().ROOT.children[+idx]); BR.sel[1] = 0; BR.path.push(brHere().children[0]); BR.sel[2] = 0; } else browseOpen('nintendo');
      BR.sys = brFocused(); BR.row = 0; slOpenState(0, m); BR.snapFor = null; S.scr = 'syslist';
      if (zf) S.anim = { start: 0, dur: Infinity, draw: () => slPageZoomFrame(ms) };
    }
    if (name.startsWith('stepat')) {                        // one frame of a step: stepat-r-200, stepat-l-120, optional maker: stepat-r-200-0
      const [, dir, ms, maker] = name.split('-'), d = dir === 'l' ? -1 : 1; browseOpen('consoles');
      if (maker != null) { BR.path.push(BRW().TREE.children[+maker]); BR.sel[2] = 0; }
      const lvl = BR.path.length - 1, cards = brHere().children, from = BR.sel[lvl] || 0, { to, leave, enter } = bstepRoles(cards.length, from, d);
      S.anim = { start: 0, dur: Infinity, draw: () => browseStepFrame(cards, from, to, d, leave, enter, +ms) };
    }
    if (name.startsWith('lvl-')) { const idx = +name.split('-')[1]; browseOpen('launcher'); BR.sel[0] = idx; BR.path.push(BRW().ROOT.children[idx]); BR.sel[1] = 0; }
    if (name.startsWith('trickat')) { const [, msPart, idxPart] = name.split('-'); browseOpen('launcher'); BR.sel[1] = 0; if (idxPart != null) BR.sel[0] = +idxPart; const st = trickState(1), ms = +msPart; S.anim = { start: 0, dur: Infinity, draw: () => browseTrickFrame(st, ms) }; }
    if (name.startsWith('backat')) { browseOpen('consoles'); const st = trickState(-1), ms = +name.split('-')[1]; S.anim = { start: 0, dur: Infinity, draw: () => browseTrickFrame(st, ms) }; }
    if (name === 'system' || name.startsWith('sysat')) { browseOpen('nintendo'); BR.sys = brFocused(); BR.row = 0; S.scr = 'system';
      if (name.startsWith('sysat')) { const ms = +name.split('-')[1]; S.anim = { start: 0, dur: Infinity, draw: () => sysZoomFrame(ms) }; } }
    const ask = (kind, extra = {}) => { askOpen({ kind, ...extra }); S.ask.instant = true; };
    if (name === 'ask-exit') { S.sel = 5; ask('exit-to-mister', { anchor: rowAnchor(5) }); }
    if (name === 'ask-refresh') { S.sel = 6; ask('refresh-database', { anchor: rowAnchor(6) }); }
    if (name === 'ask-unavailable') { S.sel = 6; ask('database-refresh-unavailable', { anchor: rowAnchor(6) }); }
    if (name === 'ask-restart') { S.sel = 5; ask('restart', { anchor: null }); }
    if (name === 'ask-resolution') { S.sel = 0; ddOpen(); S.anim = null; S.dd.hi = 8; ddSelect(); S.anim = null; S.ask.instant = true; }
    if (name === 'ask-library' || name === 'ask-failed') { S.scr = 'launcher'; ask(name === 'ask-library' ? 'library-changed' : 'library-update-failed', { anchor: null }); }
    if (name === 'ask-fav') { S.card = 'arcade'; S.scr = 'arcade'; S.arc = S.arcScroll = 5; S.shot = shotName(5); S.shotAt = 0; askGame(); S.ask.instant = true; }
    if (name === 'arcade' || name === 'arcade-v') { S.card = 'arcade'; S.scr = 'arcade'; S.arc = S.arcScroll = name === 'arcade-v' ? 8 : 5; S.shot = shotName(S.arc); S.shotAt = 0; }
    if (name === 'arclauncher') { S.scr = 'launcher'; S.card = 'arcade'; }
    if (name === 'archub') { S.card = 'arcade'; S.scr = 'arcade'; S.amode = 'hub'; S.arc = S.arcScroll = 5; S.shot = shotName(5); S.shotAt = 0; }
    if (name === 'search' || name === 'search-results') { S.card = 'arcade'; S.scr = 'search'; SQ.q = '194'; sfilter();
      if (name === 'search-results') { SQ.zone = 'results'; SQ.res = 1; }
      S.shot = shotName(SQ.zone === 'results' ? fullGames().indexOf(SQ.results[SQ.res]) : S.arc); S.shotAt = 0; }
    if (name.startsWith('searchat')) { S.card = 'arcade'; S.shot = shotName(S.arc); S.shotAt = 0; sfilter(); const ms = +name.split('-')[1]; S.anim = { start: 0, dur: Infinity, draw: () => searchFrame(ms) }; }
    if (name.startsWith('arczoomat')) { S.card = 'arcade'; S.shot = shotName(S.arc); S.shotAt = 0; const ms = +name.split('-')[1]; S.anim = { start: 0, dur: Infinity, draw: () => arcZoomFrame(ms) }; }
    // Single frames for review: zoomat-<ms>, pushat-<ms>
    const at = +(name.split('-')[1] || 0);
    if (name.startsWith('zoomat')) S.anim = { start: 0, dur: Infinity, draw: () => zoomFrame(at) };
    if (name.startsWith('pushat')) { S.sel = 7; S.anim = { start: 0, dur: Infinity, draw: () => pushFrame('settings', 'about', 1, at) }; }
    if (name === 'push') { S.scr = 'settings'; S.sel = 7; push('about', 1); }
  }
  return { init, key, state, _frame: now => frame(now), replay: () => { browseOpen('launcher'); BR.sel[1] = 0; setTimeout(() => browseTrick(1), 350); }, setSlow: s => { slow = s; }, zoom, get ready() { return ready; } };
})();
