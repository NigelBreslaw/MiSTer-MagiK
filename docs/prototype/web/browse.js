// Collection browsing (HDMI): Consoles -> maker -> system, in the launcher style.
//  * Generic card: the launcher's no-artwork card (tinted frame in the
//    collection colour, pixel category icon, name, game count).
//  * Browse views reuse the launcher layout: breadcrumb in the header, the
//    left column describes where you are, the carousel shows the choices.
//  * One system view for every system (About layout), entered by the same
//    card zoom as Settings and Arcade. No breadcrumb there.
window.BROWSE = (() => {
  const ACC = '#5a71e7', ACC_DEEP = '#3149bd';            // Consoles blue, sampled from the launcher card
  const sys = (name, full, maker, year, gen, media, ports, games, favs = 0, recent = 0) =>
    ({ name, full, maker, year, gen, media, ports, games, favs, recent });
  const TREE = { name: 'CONSOLES', kind: 'MAKERS', children: [
    { name: 'ATARI', children: [sys('2600', 'ATARI 2600', 'ATARI', 1977, '8-BIT', 'CARTRIDGE', 2, 824), sys('5200', 'ATARI 5200', 'ATARI', 1982, '8-BIT', 'CARTRIDGE', 4, 129), sys('7800', 'ATARI 7800', 'ATARI', 1986, '8-BIT', 'CARTRIDGE', 2, 400)] },
    { name: 'SEGA', children: [sys('SG-1000', 'SG-1000', 'SEGA', 1983, '8-BIT', 'CARTRIDGE', 2, 88), sys('MASTER SYSTEM', 'MASTER SYSTEM', 'SEGA', 1986, '8-BIT', 'CARTRIDGE', 2, 412), sys('MEGA DRIVE', 'MEGA DRIVE', 'SEGA', 1988, '16-BIT', 'CARTRIDGE', 2, 1164, 2), sys('MEGA-CD', 'MEGA-CD', 'SEGA', 1991, '16-BIT', 'CD-ROM', 2, 197), sys('32X', 'SEGA 32X', 'SEGA', 1994, '32-BIT', 'CARTRIDGE', 2, 36)] },
    { name: 'PLAYSTATION', children: [sys('PLAYSTATION', 'PLAYSTATION', 'SONY', 1994, '32-BIT', 'CD-ROM', 2, 1291)] },
    { name: 'NINTENDO', children: [sys('NES', 'NINTENDO ENTERTAINMENT SYSTEM', 'NINTENDO', 1983, '8-BIT', 'CARTRIDGE', 2, 1729), sys('FAMICOM DISK', 'FAMICOM DISK SYSTEM', 'NINTENDO', 1986, '8-BIT', 'DISK CARD', 2, 196), sys('SNES', 'SUPER NINTENDO', 'NINTENDO', 1990, '16-BIT', 'CARTRIDGE', 2, 1857, 1, 4), sys('NINTENDO 64', 'NINTENDO 64', 'NINTENDO', 1996, '64-BIT', 'CARTRIDGE', 4, 388)] },
    { name: 'NEC', children: [sys('PC ENGINE', 'PC ENGINE', 'NEC', 1987, '8-BIT', 'HUCARD', 5, 694), sys('PC ENGINE CD', 'PC ENGINE CD-ROM2', 'NEC', 1988, '8-BIT', 'CD-ROM', 5, 402), sys('SUPERGRAFX', 'SUPERGRAFX', 'NEC', 1989, '8-BIT', 'HUCARD', 5, 7)] },
    { name: 'SNK', children: [sys('NEO GEO', 'NEO GEO AES', 'SNK', 1990, '16-BIT', 'CARTRIDGE', 2, 184)] },
  ] };
  const games = n => n.children ? n.children.reduce((a, c) => a + games(c), 0) : n.games;
  const favs = n => n.children ? n.children.reduce((a, c) => a + favs(c), 0) : n.favs;
  const systems = n => n.children ? n.children.reduce((a, c) => a + systems(c), 0) : 1;
  TREE.kind = 'MAKERS'; TREE.children.forEach(m => { m.kind = 'SYSTEMS'; });
  // The launcher itself: the real card art (apps/mister/assets/ui/launcher-cards)
  // with each collection's neon colour, so the trick starts where it will on the device.
  const coll = (name, art, colour, games, extra = {}) => ({ name, art, colour, games, favs: 0, ...extra });
  TREE.art = 'lc-02_consoles.png'; TREE.colour = ACC;
  const COMPUTERS = { name: 'COMPUTERS', art: 'lc-03_computers.png', colour: '#e6c23a', children: [
    { name: 'COMMODORE', children: [sys('C64', 'COMMODORE 64', 'COMMODORE', 1982, '8-BIT', 'DISK / TAPE', 2, 6120, 3, 5), sys('AMIGA', 'COMMODORE AMIGA', 'COMMODORE', 1985, '32-BIT', 'FLOPPY DISK', 2, 4210, 1),
      sys('VIC-20', 'VIC-20', 'COMMODORE', 1980, '8-BIT', 'CARTRIDGE / TAPE', 2, 610), sys('C128', 'COMMODORE 128', 'COMMODORE', 1985, '8-BIT', 'DISK / TAPE', 2, 230), sys('PET 2001', 'COMMODORE PET 2001', 'COMMODORE', 1977, '8-BIT', 'TAPE', 0, 90)] },
    { name: 'ATARI', children: [sys('ATARI 800', 'ATARI 800', 'ATARI', 1979, '8-BIT', 'CARTRIDGE / DISK', 4, 1420), sys('ATARI ST', 'ATARI ST', 'ATARI', 1985, '16-BIT', 'FLOPPY DISK', 2, 1880)] },
    { name: 'SINCLAIR', children: [sys('ZX SPECTRUM', 'ZX SPECTRUM', 'SINCLAIR', 1982, '8-BIT', 'TAPE', 1, 2200, 2), sys('ZX81', 'ZX81', 'SINCLAIR', 1981, '8-BIT', 'TAPE', 0, 210)] },
    { name: 'ACORN', children: [sys('BBC MICRO', 'BBC MICRO', 'ACORN', 1981, '8-BIT', 'DISK / TAPE', 1, 480), sys('ARCHIMEDES', 'ACORN ARCHIMEDES', 'ACORN', 1987, '32-BIT', 'FLOPPY DISK', 1, 260)] },
    { name: 'MSX', children: [sys('MSX', 'MSX', 'MSX', 1983, '8-BIT', 'CARTRIDGE / TAPE', 2, 690), sys('MSX2', 'MSX2', 'MSX', 1985, '8-BIT', 'CARTRIDGE / DISK', 2, 415)] },
    { name: 'APPLE', children: [sys('APPLE II', 'APPLE II', 'APPLE', 1977, '8-BIT', 'FLOPPY DISK', 0, 300), sys('MAC PLUS', 'MACINTOSH PLUS', 'APPLE', 1986, '16-BIT', 'FLOPPY DISK', 0, 120)] },
    { name: 'SHARP', children: [sys('X68000', 'SHARP X68000', 'SHARP', 1987, '16-BIT', 'FLOPPY DISK', 2, 180)] },
  ] };
  const HANDHELDS = { name: 'HANDHELDS', art: 'lc-04_handhelds.png', colour: '#35c48f', children: [
    { name: 'NINTENDO', children: [sys('GAME BOY', 'GAME BOY', 'NINTENDO', 1989, '8-BIT', 'CARTRIDGE', 1, 1120, 2), sys('GAME BOY COLOR', 'GAME BOY COLOR', 'NINTENDO', 1998, '8-BIT', 'CARTRIDGE', 1, 1264),
      sys('GAME BOY ADVANCE', 'GAME BOY ADVANCE', 'NINTENDO', 2001, '32-BIT', 'CARTRIDGE', 1, 1301, 1, 3), sys('POKEMON MINI', 'POKEMON MINI', 'NINTENDO', 2001, '8-BIT', 'CARTRIDGE', 1, 27)] },
    { name: 'SEGA', children: [sys('GAME GEAR', 'SEGA GAME GEAR', 'SEGA', 1990, '8-BIT', 'CARTRIDGE', 1, 712)] },
    { name: 'ATARI', children: [sys('LYNX', 'ATARI LYNX', 'ATARI', 1989, '16-BIT', 'CARTRIDGE', 1, 89)] },
    { name: 'SNK', children: [sys('NEO GEO POCKET', 'NEO GEO POCKET', 'SNK', 1998, '16-BIT', 'CARTRIDGE', 1, 79), sys('NGP COLOR', 'NEO GEO POCKET COLOR', 'SNK', 1999, '16-BIT', 'CARTRIDGE', 1, 82)] },
    { name: 'BANDAI', children: [sys('WONDERSWAN', 'WONDERSWAN', 'BANDAI', 1999, '16-BIT', 'CARTRIDGE', 1, 92), sys('WONDERSWAN COLOR', 'WONDERSWAN COLOR', 'BANDAI', 2000, '16-BIT', 'CARTRIDGE', 1, 111)] },
    { name: 'WATARA', children: [sys('SUPERVISION', 'WATARA SUPERVISION', 'WATARA', 1992, '8-BIT', 'CARTRIDGE', 1, 62)] },
    { name: 'CREATRONIC', children: [sys('MEGA DUCK', 'MEGA DUCK', 'CREATRONIC', 1993, '8-BIT', 'CARTRIDGE', 1, 40)] },
  ] };
  // Each collection has its own colour and pixel icon; everything below it inherits both.
  const paint = (n, acc, icon) => { n.acc = acc; n.icon = icon; (n.children || []).forEach(c => paint(c, acc, icon)); };
  [[TREE, ACC, 'pad'], [COMPUTERS, COMPUTERS.colour, 'computer'], [HANDHELDS, HANDHELDS.colour, 'handheld']].forEach(([T, acc, icon]) => {
    T.kind = 'MAKERS'; T.children.forEach(m => { m.kind = 'SYSTEMS'; }); paint(T, acc, icon); });
  const ROOT = { name: 'MISTER MAGIK', kind: 'COLLECTIONS', root: true, children: [
    coll('SETTINGS', 'lc-06_settings.png', '#9c86e7'), coll('ARCADE', 'lc-01_arcade.png', '#e7695a', 999),
    TREE, COMPUTERS, HANDHELDS,
    coll('FAVOURITES', 'lc-05_favourites.png', '#e5467f', 1)] };
  const art = {};

  // Pixel category icons (same idea as the launcher's arcade fallback).
  const GAMEPAD = ['0011111111111100', '0111111111111110', '1110111111111011', '1100011111110101', '1110111111111011',
    '1111111111111111', '1111110000111111', '1111100000011111', '0111000000001110', '0010000000000100'];
  // '1' solid body, '2' dim glass in the collection colour, '0' empty.
  // A 5:4 monitor (10x8 body, 8x6 glass), neck and base, centred in the 16x10 grid.
  const COMPUTER = ['0001111111111000', '0001222222221000', '0001222222221000', '0001222222221000', '0001222222221000',
    '0001222222221000', '0001222222221000', '0001111111101000', '0000001111000000', '0000111111110000'];
  const HANDHELD = ['0111111111111110', '1111111111111111', '1111122222211111', '1101122222211011', '1000122222210111',
    '1101122222211111', '1111122222211111', '1111111111111111', '1111111111111111', '0111111111111110'];
  const iconOf = n => n.icon === 'computer' ? COMPUTER : n.icon === 'handheld' ? HANDHELD : GAMEPAD;
  const accOf = n => n.acc || ACC;
  const deepOf = n => n.acc && n.acc !== ACC ? mix('#000000', n.acc, .6) : ACC_DEEP;
  const rgbaOf = (hex, a) => { const c = [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16)); return `rgba(${c.join(',')},${a})`; };
  const mix = (a, b, k) => { const p = h => [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16)); const x = p(a), y = p(b);
    return `rgb(${x.map((v, i) => Math.round(v + (y[i] - v) * k)).join(',')})`; };

  // The generic card, 180x252 like every launcher card.
  function cardImage(node) {
    const c = document.createElement('canvas'); c.width = 180; c.height = 252;
    const ACC = accOf(node), ACC_DEEP = deepOf(node), GAMEPAD = iconOf(node);
    const g = c.getContext('2d'), W = 180, H = 252, r = 10;
    const path = (x, y, w, h, rr) => { g.beginPath(); g.moveTo(x + rr, y); g.arcTo(x + w, y, x + w, y + h, rr); g.arcTo(x + w, y + h, x, y + h, rr); g.arcTo(x, y + h, x, y, rr); g.arcTo(x, y, x + w, y, rr); g.closePath(); };
    path(0, 0, W, H, r); g.save(); g.clip();
    const bg = g.createLinearGradient(0, 0, 0, H);
    bg.addColorStop(0, mix('#0c161e', ACC, .34)); bg.addColorStop(.55, mix('#0c161e', ACC, .16)); bg.addColorStop(1, mix('#05080c', ACC, .08));
    g.fillStyle = bg; g.fillRect(0, 0, W, H);
    // icon: cream pixels with a coloured drop, centred in the art area
    const s = 5, iw = 16 * s, ix = (W - iw) / 2, iy = Math.round(H * .22);
    [[3, ACC_DEEP], [0, '#eee8d5']].forEach(([o, col]) => {
      GAMEPAD.forEach((row, y) => [...row].forEach((b, x) => { if (b === '0') return;
        g.fillStyle = b === '2' && !o ? mix('#05070c', ACC, .5) : col; g.fillRect(ix + x * s + o, iy + y * s + o, s, s); })); });
    g.restore();
    path(1.5, 1.5, W - 3, H - 3, r - 1); g.lineWidth = 3; g.strokeStyle = ACC; g.stroke();
    g.fillStyle = '#eee8d5'; g.textAlign = 'center'; g.textBaseline = 'top';
    g.font = '16px Nocive'; g.fillText(node.name, W / 2, Math.round(H * .73) - 8);
    g.font = '16px Xerxes'; g.fillText(`${games(node)} GAMES`, W / 2, Math.round(H * .86) - 8);
    return c.toDataURL();
  }

  let winEl, root, carEl, leftEl, sysEl, crumbEl, outline, faceEl, active = false, busy = false;
  const path = [ROOT], sel = [2, 0, 0];                   // demo: Consoles on the launcher
  let view = 'browse', sysRow = 0, curSys = null;
  const here = () => path[path.length - 1];
  const focused = () => here().children[sel[path.length - 1] || 0];

  function css() {
    const st = document.createElement('style');
    st.textContent = `
    #browse{position:absolute;inset:0;background:#000;display:none;--acc:${ACC}}
    #browse.on{display:block}
    #browse .rule{position:absolute;height:1px;background:var(--rule)}
    #browse .vr{position:absolute;left:265px;top:95px;width:1px;height:384px;background:var(--rule)}
    #browse .big{font-family:Jersey;font-size:48px;line-height:1;white-space:nowrap}
    #browse .n{font-family:Jersey;font-size:56px;line-height:1;white-space:nowrap}
    #bcar{position:absolute;inset:0;perspective:1100px;perspective-origin:610px 284px}
    #bcar .c3{position:absolute;left:0;top:0;width:180px;height:252px;transform-style:preserve-3d;transform-origin:90px 126px}
    #bcar .c3 img{position:absolute;inset:0;width:180px;height:252px;backface-visibility:hidden;
      -webkit-box-reflect:below 4px linear-gradient(transparent 70%,rgba(255,255,255,.18))}
    #bcar .c3 img.b{transform:rotateY(180deg)}
    #browse .crumb .up{color:var(--muted)}
    #browse .brow{position:absolute;left:296px;width:638px;height:36px}
    #browse .brow .lbl{position:absolute;left:12px;top:11px}
    #browse .brow .val{position:absolute;right:14px;top:11px;color:var(--muted)}
    #browse .brow .sep{position:absolute;left:12px;right:0;bottom:0;height:1px;background:var(--rule)}
    #browse .brow.info .lbl{color:#c9c3b3}
    #browse .brow.focus{background:linear-gradient(90deg,${mix('#000000', ACC, .26)} 0%,${mix('#000000', ACC, .1)} 60%,rgba(0,0,0,.6) 100%)}
    #browse .brow.focus::before{content:"";position:absolute;left:0;top:0;bottom:0;width:4px;background:var(--acc);box-shadow:0 0 8px ${ACC_DEEP}}
    #browse .brow.focus .lbl,#browse .brow.focus .chev{color:var(--acc)} #browse .brow.focus .val{color:var(--cream)}
    #browse .brow.focus .sep,#browse .brow.last .sep{background:transparent}
    #browse .grp{position:absolute;left:308px}
    #browse .fx{will-change:transform,opacity}
    #browse .sysname{font-family:Jersey;font-size:64px;line-height:58px;width:440px;white-space:normal;color:var(--cream)}
    #browse .tile{position:absolute;top:318px;width:148px;height:128px;border-radius:10px;box-sizing:border-box;
      border:2px solid ${mix('#000000', ACC, .45)};background:linear-gradient(180deg,${mix('#0c161e', ACC, .24)},${mix('#05080c', ACC, .06)});
      transition:transform .16s cubic-bezier(.2,.8,.2,1),box-shadow .16s,border-color .16s}
    #browse .tile .num{position:absolute;left:14px;top:12px;font-family:Jersey;font-size:52px;line-height:1;color:var(--cream)}
    #browse .tile .lbl{position:absolute;left:14px;bottom:14px;color:var(--muted)}
    #browse .tile.on{border-color:${ACC};box-shadow:0 0 20px ${ACC_DEEP},0 14px 26px #000;transform:translateY(-8px)}
    #browse .tile.on .lbl{color:var(--cream)}
    #browse .tile.on::after{content:"";position:absolute;left:14px;right:14px;bottom:0;height:3px;background:${ACC}}`;
    document.head.append(st);
  }

  function build(stage) {
    css();
    root = document.createElement('div'); root.id = 'browse';
    root.innerHTML = `
      <div class="a h crumb" id="bcrumb" style="left:26px;top:21px"></div>
      <div class="a h" style="left:874px;top:26px">17:33</div>
      <div class="rule" style="left:26px;top:76px;width:908px"></div>
      <div class="rule" style="left:26px;top:500px;width:908px"></div>
      <div id="bbrowse">
        <div id="bleft"></div>
        <div class="vr fx"></div>
        <div class="a m muted fx" id="bkind" style="left:296px;top:104px"></div>
        <div id="bcar"></div>
        <div class="a m foot fx" style="left:30px;top:518px">A &nbsp;OPEN</div>
        <div class="a m foot fx" style="left:130px;top:518px">B &nbsp;BACK</div>
        <div class="a m muted fx" style="left:600px;top:518px">LEFT RIGHT &nbsp;BROWSE CARDS</div>
      </div>
      <div id="bwin" style="position:absolute;left:520px;top:158px;width:180px;height:252px;border-radius:11px;background:#000;opacity:0;pointer-events:none"></div>
      <div id="bsys"></div>
      <div id="bface" style="position:absolute;left:521px;top:159px;width:178px;height:250px;border-radius:10px;background:#000 center/180px 252px;opacity:0;pointer-events:none"></div>
      <div id="bout" style="position:absolute;left:520px;top:158px;width:180px;height:252px;border:3px solid ${ACC};border-radius:11px;box-sizing:border-box;box-shadow:0 0 14px ${ACC_DEEP};opacity:0;pointer-events:none"></div>`;
    stage.append(root);
    carEl = root.querySelector('#bcar'); leftEl = root.querySelector('#bleft'); sysEl = root.querySelector('#bsys');
    crumbEl = root.querySelector('#bcrumb'); winEl = root.querySelector('#bwin'); outline = root.querySelector('#bout'); faceEl = root.querySelector('#bface');
  }

  // ---------- browse view ----------
  function crumb(p = path) {
    const names = p.length > 1 ? p.slice(1).map(n => n.name) : ['MISTER MAGIK'];
    crumbEl.innerHTML = names.map((n, i) => i < names.length - 1 ? `<span class="up">${n} / </span>` : n).join('');
  }
  function renderLeft() {
    const n = here();
    if (n.root) {
      leftEl.innerHTML = `
      <div class="a m muted fx" style="left:29px;top:104px">YOUR LIBRARY</div>
      <div class="a big fx" style="left:28px;top:140px">35216</div>
      <div class="a m muted fx" style="left:29px;top:209px">GAMES READY TO PLAY</div>
      <div class="rule fx" style="left:29px;top:239px;width:211px"></div>
      <div class="a big fx" style="left:28px;top:266px">77</div>
      <div class="a big fx" style="left:150px;top:266px">1</div>
      <div class="a m muted fx" style="left:29px;top:314px">COLLECTIONS</div>
      <div class="a m muted fx" style="left:150px;top:314px">FAVOURITES</div>
      <div class="rule fx" style="left:29px;top:340px;width:211px"></div>
      ${['#e7695a', '#e6c23a', '#35c48f', '#3d8fd6'].map((c, i) => `<div class="fx" style="position:absolute;left:${29 + i * 54}px;top:438px;width:48px;height:5px;background:${c}"></div>`).join('')}`;
      root.querySelector('#bkind').textContent = n.kind; return;
    }
    leftEl.innerHTML = `
      <div class="a m muted fx" style="left:29px;top:104px">${n.name === 'CONSOLES' ? 'CONSOLES' : n.name}</div>
      <div class="a big fx" style="left:28px;top:140px">${games(n)}</div>
      <div class="a m muted fx" style="left:29px;top:209px">GAMES READY TO PLAY</div>
      <div class="rule fx" style="left:29px;top:239px;width:211px"></div>
      <div class="a big fx" style="left:28px;top:266px">${n.children.length}</div>
      <div class="a big fx" style="left:150px;top:266px">${favs(n)}</div>
      <div class="a m muted fx" style="left:29px;top:314px">${n.kind}</div>
      <div class="a m muted fx" style="left:150px;top:314px">FAVOURITES</div>
      <div class="rule fx" style="left:29px;top:340px;width:211px"></div>
      <div class="fx" style="position:absolute;left:29px;top:438px;width:211px;height:5px;background:${accOf(n)}"></div>`;
    root.querySelector('#bkind').textContent = n.kind;
  }
  // ---------- 3D card carousel (the launcher's look: neon glow, edge-on flips) ----------
  // Every card is a two-faced element: front = generic card, back = MagiK card
  // back. Positions are {x,y} centres plus depth, scale, Y/Z rotation and a
  // brightness for the dimmed side cards.
  function backImage(colour = ACC) {
    backImage.cache = backImage.cache || {};
    if (backImage.cache[colour]) return backImage.cache[colour];
    const ACC = colour;
    const c = document.createElement('canvas'); c.width = 180; c.height = 252;
    const g = c.getContext('2d'), W = 180, H = 252;
    const path = (x, y, w, h, rr) => { g.beginPath(); g.moveTo(x + rr, y); g.arcTo(x + w, y, x + w, y + h, rr); g.arcTo(x + w, y + h, x, y + h, rr); g.arcTo(x, y + h, x, y, rr); g.arcTo(x, y, x + w, y, rr); g.closePath(); };
    path(0, 0, W, H, 10); g.save(); g.clip();
    g.fillStyle = mix('#05070c', ACC, .12); g.fillRect(0, 0, W, H);
    g.strokeStyle = mix('#05070c', ACC, .32); g.lineWidth = 2;
    for (let i = -H; i < W + H; i += 12) { g.beginPath(); g.moveTo(i, 0); g.lineTo(i + H, H); g.stroke(); g.beginPath(); g.moveTo(i + H, 0); g.lineTo(i, H); g.stroke(); }
    g.restore();
    path(10, 10, W - 20, H - 20, 6); g.lineWidth = 1; g.strokeStyle = mix('#05070c', ACC, .6); g.stroke();
    // emblem: a diamond with the MagiK M
    g.save(); g.translate(W / 2, H / 2); g.rotate(Math.PI / 4); g.fillStyle = '#05070c'; g.fillRect(-38, -38, 76, 76);
    g.lineWidth = 3; g.strokeStyle = ACC; g.strokeRect(-38, -38, 76, 76); g.restore();
    g.fillStyle = '#eee8d5'; g.font = '72px Jersey'; g.textAlign = 'center'; g.textBaseline = 'middle'; g.fillText('M', W / 2, H / 2 + 4);
    path(1.5, 1.5, W - 3, H - 3, 9); g.lineWidth = 3; g.strokeStyle = ACC; g.stroke();
    return (backImage.cache[colour] = c.toDataURL());
  }
  const faceSrc = n => n.img || (n.img = n.art ? launcherCard(n) : cardImage(n));
  function launcherCard(n) {
    const c = document.createElement('canvas'); c.width = 180; c.height = 252;
    const g = c.getContext('2d'), W = 180, H = 252;
    const path = (x, y, w, h, rr) => { g.beginPath(); g.moveTo(x + rr, y); g.arcTo(x + w, y, x + w, y + h, rr); g.arcTo(x + w, y + h, x, y + h, rr); g.arcTo(x, y + h, x, y, rr); g.arcTo(x, y, x + w, y, rr); g.closePath(); };
    path(0, 0, W, H, 10); g.save(); g.clip(); g.fillStyle = '#000'; g.fillRect(0, 0, W, H);
    if (art[n.art]) g.drawImage(art[n.art], 0, 0, W, H);
    g.restore();
    path(1.5, 1.5, W - 3, H - 3, 9); g.lineWidth = 3; g.strokeStyle = n.colour; g.stroke();
    g.fillStyle = '#eee8d5'; g.textAlign = 'center'; g.textBaseline = 'top';
    g.font = '16px Nocive'; g.fillText(n.name, W / 2, Math.round(H * .73) - 8);
    const count = n.children ? games(n) : n.games;
    if (count) { g.font = '16px Xerxes'; g.fillText(`${count} GAMES`, W / 2, Math.round(H * .86) - 8); }
    return c.toDataURL();
  }
  function cardEl(node) {
    const d = document.createElement('div'); d.className = 'c3';
    d.innerHTML = `<img class="f" src="${faceSrc(node)}" alt=""><img class="b" src="${backImage(node.acc)}" alt="">`;
    d.node = node; return d;
  }
  function slot(rel) {
    const a = Math.abs(rel), side = Math.sign(rel);
    if (a === 0) return { x: 610, y: 284, z: 0, s: 1, b: 1, o: 1, glow: 1 };
    if (a === 1) return { x: 610 + side * 144, y: 284, z: -60, s: .8, b: .62, o: 1 };
    if (a === 2) return { x: 610 + side * 254, y: 284, z: -120, s: .62, b: .45, o: 1 };
    return { x: 610 + side * 254, y: 284, z: -160, s: .5, b: .3, o: 0 };
  }

  // Levels below the root: the selected card sits at the left and the rest
  // extend into the distance to the right, each smaller, dimmer and turned a
  // little. `at` places a card by where it lands on screen, undoing the
  // perspective so the row is exact.
  const OX = 610, OY = 284, PERSP = 1100;
  const at = (xs, ys, ss, depth, b, o, ry = 0, glow = 0) => {
    const f = PERSP / (PERSP + depth);
    return { x: OX + (xs - OX) / f, y: OY + (ys - OY) / f, z: -depth, s: ss / f, b, o, ry, glow };
  };
  // Each card is 10% smaller than the one in front of it, and overlaps it:
  // the next card starts STEP of the way across the card before.
  const SHRINK = .9, STEP = .85, LS = [0, 1, 2, 3, 4].map(k => Math.pow(SHRINK, k)), LB = [1, .72, .56, .42, .32], LEFT_EDGE = 292, TILT = 14;
  const LEFT = (() => {
    let left = LEFT_EDGE; const r = [];
    LS.forEach((ss, k) => { r.push(at(left + 90 * ss, 284, ss, 70 * k, LB[k], 1, k ? TILT : 0, k ? 0 : 1)); left += 180 * ss * STEP; });
    return r;
  })();
  const K = LS.length, CYCLE_MIN = 2;                      // two or more cards cycle forever; one has nothing to move
  const cyc = n => n >= CYCLE_MIN;
  const goneLeft = at(LEFT_EDGE + 90 - 12, 284, .98, 0, 1, 0, 0, 0);
  const goneRight = at(LEFT[K - 1].x + 30, 284, .34, 350, .25, 0, TILT, 0);
  const slotL = rel => rel < 0 ? goneLeft : rel >= K ? goneRight : LEFT[rel];
  // Position of card `i` relative to the selected one. Lower levels with five
  // or more cards cycle forever; smaller ones stop at their ends.
  const relOf = (i, s, n, left) => left && cyc(n) ? (((i - s) % n) + n) % n : i - s;
  const slotFor = (rel, n, left) => !left ? slot(rel) : slotL(left && cyc(n) && n > K && rel === n - 1 ? -1 : rel);
  function place(el, p, zi) {
    el.style.transform = `translate3d(${p.x - 90}px,${p.y - 126}px,${p.z || 0}px) rotateY(${p.ry || 0}deg) rotateZ(${p.rz || 0}deg) scale(${p.s})`;
    el.style.opacity = p.o == null ? 1 : p.o; el.style.zIndex = zi;
    const glow = p.glow || 0, col = (el.node && el.node.colour) || ACC;
    const f = `brightness(${p.b}) drop-shadow(0 0 ${5 + 9 * glow}px ${col}99)${glow > .5 ? ` drop-shadow(0 0 ${18 * glow}px ${col})` : ''}`;
    for (const im of el.children) im.style.filter = f;
  }
  const between = (a, b, k) => ({ x: lerp(a.x, b.x, k), y: lerp(a.y, b.y, k), z: lerp(a.z, b.z, k), s: lerp(a.s, b.s, k), b: lerp(a.b, b.b, k), o: lerp(a.o, b.o, k),
    ry: lerp(a.ry || 0, b.ry || 0, k), glow: lerp(a.glow || 0, b.glow || 0, k) });
  let cards = [];
  function renderCards() {
    carEl.innerHTML = ''; cards = here().children.map(cardEl); cards.forEach(c => carEl.append(c));
    layoutCards();
  }
  function layoutCards() {
    const s0 = sel[path.length - 1] || 0, n = cards.length, left = path.length > 1;
    cards.forEach((c, i) => {
      const rel = relOf(i, s0, n, left);
      place(c, { ...slotFor(rel, n, left), glow: i === s0 ? 1 : 0 }, 20 - Math.abs(rel));
    });
  }
  // Browsing: cards change slots and turn edge-on mid-flight, like the launcher.
  function slide(d) { if (path.length > 1) slideLeft(d); else slideRoot(d); }
  function slideRoot(d) {
    const n = cards.length, from = sel[path.length - 1] || 0, to = Math.max(0, Math.min(n - 1, from + d));
    if (to === from) return;
    sel[path.length - 1] = to;
    run(300, t => {
      const k = inOut(t / 300);
      cards.forEach((c, i) => {
        const a = slot(i - from), b = slot(i - to), p = between(a, b, k);
        p.ry = -d * 62 * Math.sin(Math.PI * k) * (i === from || i === to ? 1 : .6);
        p.glow = i === to ? k : i === from ? 1 - k : 0;
        place(c, p, 10 - Math.abs(i - (k < .5 ? from : to)));
      });
    });
  }

  // Lower levels: every card moves one slot together. Going right, the front
  // card just slides away and is clipped at the edge of the row, while a new
  // card comes out from behind the last one, turning 180 degrees from its
  // MagiK back to its face. Going left is the same motion in reverse: the end
  // card turns back and tucks behind the previous one, and the front card
  // slides back in. The end card is always the furthest back in z.
  const SLIDE_MS = 460, SLIDE_CLIP = LEFT_EDGE - 24;
  const slideOut = at(SLIDE_CLIP - 110, 284, 1, 0, 1, 1, 0, 0);                  // wholly behind the clip edge
  function slideLeft(d) {
    const n = cards.length, i0 = path.length - 1, from = sel[i0] || 0, wrap = cyc(n);
    const to = wrap ? (from + d + n) % n : Math.max(0, Math.min(n - 1, from + d));
    if (to === from) return;
    sel[i0] = to;
    const idx = j => wrap ? ((j % n) + n) % n : (j >= 0 && j < n ? j : -1);
    const Ke = Math.min(n, K);                                                  // slots in use: a short list fills only the first few
    const leaveI = idx(d > 0 ? from : from + Ke - 1), enterI = idx(d > 0 ? from + Ke : to);
    const leaving = leaveI >= 0 ? cards[leaveI] : null;
    let entering = enterI >= 0 ? cards[enterI] : null, clone = null;
    if (entering && entering === leaving) { clone = cardEl(entering.node); carEl.append(clone); entering = clone; }
    const front = d > 0 ? leaving : entering, end = d > 0 ? entering : leaving;
    carEl.style.clipPath = `inset(0 0 0 ${SLIDE_CLIP}px)`;
    run(SLIDE_MS, t => {
      const k = inOut(t / SLIDE_MS);
      cards.forEach((c, i) => {
        if (c === leaving || c === entering) return;
        const a = slotFor(relOf(i, from, n, true), n, true), b = slotFor(relOf(i, to, n, true), n, true);
        place(c, between(a, b, k), 20 - Math.abs(relOf(i, to, n, true)));
      });
      if (front) place(front, d > 0 ? between(slotL(0), slideOut, k) : between(slideOut, slotL(0), k), 30);
      if (end) {
        // Going left is the same motion run backwards. The turn runs at a steady
        // speed for the whole step, from the MagiK back to the face.
        const home = slotL(Ke - 1), tuck = { ...home, x: home.x - 10, o: 1 }, e = d > 0 ? k : 1 - k, p = between(tuck, home, e);
        p.ry = lerp(180, TILT, d > 0 ? t / SLIDE_MS : 1 - t / SLIDE_MS);
        p.o = d > 0 || k < 1 ? 1 : 0;
        place(end, p, 1);
      }
    }, () => {
      if (clone) clone.remove();
      carEl.style.clipPath = '';
      layoutCards();
    });
  }

  // ---------- the card trick: level changes ----------
  // Changing level is one continuous motion. The chosen card turns and travels
  // for the whole trick: at the midpoint (EDGE) it is edge-on and halfway from
  // its slot on the level being left to its slot on the level being entered
  // (centre on the root, the left of the row below it). Until the midpoint the
  // other cards turn and are pulled in behind it as it moves; after it the new
  // level's cards slide out from behind it, turning face-on as they land. Its
  // far side is the next level's card. Going back plays the same motion.
  const TRICK_MS = 920;
  const EDGE = TRICK_MS / 2;                               // the halfway point: the chosen card is edge-on, everything else is behind it
  const BEHIND = { x: 610, y: 284, z: -70, s: .9, b: .25, o: 1 };
  const behindOf = left => left ? { ...LEFT[0], z: -70, s: .9, b: .25, o: 1, ry: 0, glow: 0 } : BEHIND;
  function trick(dir) {
    const oldNodes = here().children, oldSel = sel[path.length - 1] || 0, oldCards = cards;
    const oldLeft = path.length > 1, oldN = oldCards.length;
    const hero = oldCards[oldSel];
    // next level
    if (dir > 0) { path.push(oldNodes[oldSel]); sel[path.length - 1] = 0; } else path.pop();
    const newNodes = here().children, newSel = sel[path.length - 1] || 0;
    const newLeft = path.length > 1, newN = newNodes.length;
    const behindOld = behindOf(oldLeft), h0 = slotFor(0, oldN, oldLeft), h1 = slotFor(0, newN, newLeft);
    const newCards = newNodes.map((n, i) => i === newSel ? null : cardEl(n));
    newCards.forEach(c => c && carEl.prepend(c));
    const heroNext = faceSrc(newNodes[newSel]);
    const [front, back] = hero.children;
    back.src = heroNext;
    const others = oldCards.filter(c => c !== hero);
    const fadeEls = [...fx(leftEl), root.querySelector('#bkind')];
    const T = TRICK_MS;
    let swapped = false;
    run(T, t => {
      // left column and breadcrumb: out while gathering, in while dealing
      const outK = inOut(win(t, 0, 260)), inK = out(win(t, T * .55, 360));
      if (t > T * .45 && !swapped) { swapped = true; crumb(); renderLeft(); }
      const colA = swapped ? inK : 1 - outK;
      [...fx(leftEl), root.querySelector('#bkind')].forEach(e => setBand(e, swapped ? 20 * (1 - inK) : -20 * outK, colA));
      crumbEl.style.opacity = swapped ? .3 + .7 * inK : 1 - .7 * outK;
      // The chosen card turns and travels for the whole trick: edge-on and
      // halfway there at the midpoint, face-on and home at the end.
      const hm = inOut(t / T), heroAt = { x: lerp(h0.x, h1.x, hm), y: lerp(h0.y, h1.y, hm) };
      // 1. gather: the others are sucked in behind the chosen card as it goes
      const behind = { ...behindOld, x: heroAt.x, y: heroAt.y };
      others.forEach(c => {
        const i = oldCards.indexOf(c), rel = relOf(i, oldSel, oldN, oldLeft), side = Math.sign(rel) || 1, a = slotFor(rel, oldN, oldLeft);
        const e = win(t, 0, EDGE), p = between(a, behind, inOut(e));
        p.ry += side * 90 * e * e;
        p.o = a.o * (1 - win(t, EDGE - 20, 20));
        place(c, p, 5 - Math.abs(rel));
      });
      // 2. the chosen card: one half-turn across the whole trick
      const ry = 180 * hm, s = 1 + .04 * Math.sin(Math.PI * t / T);
      place(hero, { x: heroAt.x, y: heroAt.y, z: 0, s, b: 1, o: 1, ry, glow: 1 }, 20);
      // 3. deal: the rest come out from behind the card
      const relN = i => relOf(i, newSel, newN, newLeft);
      const order = newCards.map((c, i) => i).filter(i => newCards[i]).sort((a, b) => Math.abs(relN(a)) - Math.abs(relN(b)) || a - b);
      order.forEach((i, n) => {
        const c = newCards[i], rel = relN(i), side = Math.sign(rel) || 1, b = slotFor(rel, newN, newLeft);
        const start = { ...BEHIND, x: heroAt.x, y: heroAt.y, o: 0 };
        // slide out from behind the card at the midpoint, turning face-on as they land
        const k = out(win(t, EDGE + Math.min(n, 5) * 20, T - EDGE - 100)), p = between(start, b, k);
        p.o = (t >= EDGE ? 1 : 0) * b.o;
        p.ry += -side * 90 * (1 - k);
        place(c, p, 10 - Math.abs(rel));
      });
    }, () => {
      front.src = heroNext; back.src = backImage(newNodes[newSel].acc);
      oldCards.forEach(c => c !== hero && c.remove());
      cards = newNodes.map((n, i) => i === newSel ? hero : newCards[i]); hero.node = newNodes[newSel];
      layoutCards();
      [...fx(leftEl), root.querySelector('#bkind')].forEach(e => setBand(e, 0, 1)); crumbEl.style.opacity = 1;
    });
  }
  function renderBrowse() { crumb(); renderLeft(); renderCards(); }

  // ---------- system view: a hero page, one layout for every system ----------
  // The right side is the hero: today a lit, extruded version of the card's
  // pixel icon; later a Blender render of the console in the same box
  // (HERO: 560x470 at 420,66, object centred on HERO_C). The left is a
  // title block and three destination tiles, moved with LEFT/RIGHT.
  const HERO = { x: 420, y: 66, w: 560, h: 470 }, HERO_C = { x: 280, y: 200 }, VOX = 24;
  function heroImage(s) {
    if (s.hero) return s.hero;
    const c = document.createElement('canvas'); c.width = HERO.w; c.height = HERO.h;
    const ACC = accOf(s), GAMEPAD = iconOf(s);
    const g = c.getContext('2d');
    const glow = g.createRadialGradient(HERO_C.x, HERO_C.y + 20, 10, HERO_C.x, HERO_C.y + 20, 300);
    glow.addColorStop(0, rgbaOf(ACC, .34)); glow.addColorStop(.55, rgbaOf(ACC, .12)); glow.addColorStop(1, 'rgba(0,0,0,0)');
    g.fillStyle = glow; g.fillRect(0, 0, HERO.w, HERO.h);
    // contact shadow and a thin rim of light on the "floor"
    g.save(); g.translate(HERO_C.x + 20, HERO_C.y + 176); g.scale(1, .12);
    const fl = g.createRadialGradient(0, 0, 0, 0, 0, 240); fl.addColorStop(0, 'rgba(0,0,0,.85)'); fl.addColorStop(.7, 'rgba(0,0,0,.4)'); fl.addColorStop(1, 'rgba(0,0,0,0)');
    g.fillStyle = fl; g.beginPath(); g.arc(0, 0, 240, 0, 7); g.fill(); g.restore();
    const ox = HERO_C.x - 8 * VOX, oy = HERO_C.y - 5 * VOX;
    const each = f => GAMEPAD.forEach((row, y) => [...row].forEach((b, x) => { if (b !== '0') f(ox + x * VOX, oy + y * VOX, b); }));
    for (let d = 16; d >= 1; d--) { g.fillStyle = mix('#05070f', ACC, .10 + (16 - d) * .014); each((x, y) => g.fillRect(x + d * 1.4, y + d * 1.6, VOX, VOX)); }
    each((x, y, b) => {
      const t = (y - oy) / (10 * VOX);
      g.fillStyle = b === '2' ? mix(mix('#05070f', ACC, .55), mix('#05070f', ACC, .18), t) : mix('#e6e3f2', '#8c89a8', t * .7); g.fillRect(x, y, VOX, VOX);
      g.fillStyle = b === '2' ? 'rgba(255,255,255,.14)' : 'rgba(255,255,255,.55)'; g.fillRect(x, y, VOX, 3); g.fillRect(x, y, 3, VOX);
      g.fillStyle = 'rgba(20,20,48,.28)'; g.fillRect(x, y + VOX - 3, VOX, 3); g.fillRect(x + VOX - 3, y, 3, VOX);
    });
    // rim light from the collection colour, bottom right edges
    g.globalCompositeOperation = 'source-atop'; const rim = g.createLinearGradient(ox, oy, ox + 16 * VOX, oy + 10 * VOX);
    rim.addColorStop(0, rgbaOf(ACC, 0)); rim.addColorStop(1, rgbaOf(ACC, .35)); g.fillStyle = rim; g.fillRect(0, 0, HERO.w, HERO.h);
    g.globalCompositeOperation = 'source-over';
    s.hero = c; return c;
  }
  const TILES = s => [
    { l: 'GAMES', n: s.games, cap: `BROWSE ALL ${s.games} ${s.full} GAMES` },
    { l: 'RECENT', n: s.recent, cap: s.recent ? `PICK UP WHERE YOU LEFT OFF` : 'NOTHING PLAYED YET' },
    { l: 'FAVOURITES', n: s.favs, cap: s.favs ? `${s.favs} SAVED FAVOURITE${s.favs === 1 ? '' : 'S'}` : 'NO FAVOURITES YET' }];
  function renderSystem(s) {
    const tiles = TILES(s);
    sysEl.innerHTML = `
      <div id="bhero" style="position:absolute;left:${HERO.x}px;top:${HERO.y}px;width:${HERO.w}px;height:${HERO.h}px;transform-origin:${HERO_C.x}px ${HERO_C.y}px"></div>
      <div class="a m muted fx" style="left:40px;top:104px">${s.maker} &nbsp;/&nbsp; ${s.year} &nbsp;/&nbsp; ${s.gen}</div>
      <div class="a fx sysname" style="left:37px;top:126px">${s.full}</div>
      <div class="a m fx" id="bcount" style="left:40px;top:0">${s.games} GAMES READY TO PLAY</div>
      ${tiles.map((t, i) => `<div class="tile fx${i === sysRow ? ' on' : ''}" style="left:${40 + i * 160}px">
        <span class="num">${t.n}</span><span class="lbl h">${t.l}</span></div>`).join('')}
      <div class="a m fx" id="bcap" style="left:40px;top:470px;color:#c9c3b3">${tiles[sysRow].cap}</div>
      <div class="a m foot fx" style="left:30px;top:518px">A &nbsp;OPEN</div>
      <div class="a m foot fx" style="left:130px;top:518px">B &nbsp;BACK</div>
      <div class="a m fx" style="left:612px;top:518px">LEFT RIGHT &nbsp;MOVE</div>`;
    sysEl.querySelector('#bhero').append(heroImage(s));
    // the name may wrap to two lines; the count follows it
    const nm = sysEl.querySelector('.sysname');
    sysEl.querySelector('#bcount').style.top = (126 + nm.offsetHeight + 10) + 'px';
  }
  function moveTile(d) {
    const n = Math.max(0, Math.min(2, sysRow + d)); if (n === sysRow) return; sysRow = n;
    sysEl.querySelectorAll('.tile').forEach((t, i) => t.classList.toggle('on', i === sysRow));
    const cap = sysEl.querySelector('#bcap'); cap.textContent = TILES(curSys)[sysRow].cap;
    cap.animate([{ opacity: 0, transform: 'translateX(10px)' }, { opacity: 1, transform: 'none' }], { duration: 180, easing: 'cubic-bezier(.2,.8,.2,1)' });
  }
  // Hero during the card zoom: the card's icon (80px wide, centred at
  // 610,238 on the card) grows into the hero object.
  let CX = 610;                                            // where the focused card sits
  function heroFrame(t) {
    const h = sysEl.querySelector('#bhero'); if (!h) return;
    const p = inOut(win(t, 60, 600)), s0 = 80 / (16 * VOX);
    const sx = HERO.x + HERO_C.x, sy = HERO.y + HERO_C.y;
    h.style.transform = `translate(${lerp(CX - sx, 0, p)}px,${lerp(238 - sy, 0, p)}px) scale(${lerp(s0, 1, p)})`;
    h.style.opacity = win(t, 40, 120);
  }
  const lerp = (a, b, p) => a + (b - a) * p;

  // ---------- transitions (frame functions, so reverse mirrors exactly) ----------
  const clamp01 = v => v < 0 ? 0 : v > 1 ? 1 : v;
  const inOut = t => t < .5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
  const out = t => 1 - Math.pow(1 - t, 4);
  const win = (t, at, d) => clamp01((t - at) / d);
  function run(dur, frame, done) {
    busy = true; const t0 = performance.now(), sl = typeof slow === 'number' ? slow : 1;
    const step = now => { const t = Math.min(dur, (now - t0) / sl); frame(t); if (t < dur) requestAnimationFrame(step); else { busy = false; done && done(); } };
    requestAnimationFrame(step);
  }
  const fx = el => [...el.querySelectorAll('.fx')];
  const setBand = (el, dx, a) => { el.style.transform = dx ? `translateX(${dx}px)` : ''; el.style.opacity = a; };

  // Card zoom into the system view: the outline zooms past the edges in the
  // collection colour, the browse view fades, the system rows deal in.
  const ZOOM_MS = 900, ZMAX = 8;
  function zoomFrame(t) {
    const z = Math.exp(Math.log(ZMAX) * inOut(win(t, 0, 680)));
    const browseA = 1 - win(t, 90, 260);
    fx(root.querySelector('#bbrowse')).concat(carEl).forEach(e => { e.style.opacity = browseA; e.style.transform = ''; });
    crumbEl.style.opacity = t < 300 ? 1 - win(t, 90, 200) : win(t, 300, 200);
    if (t >= 300) crumbEl.innerHTML = 'MISTER MAGIK'; else crumb();
    winEl.style.opacity = t > 0 && t < ZOOM_MS ? 1 : 0; winEl.style.transform = `scale(${z})`;
    faceEl.style.opacity = t > 0 ? 1 - win(t, 40, 200) : 0;
    faceEl.style.transform = `scale(${z})`;
    outline.style.opacity = Math.max(0, 1 - 1.25 * Math.pow(win(t, 0, 680), 2)) * (t > 0 ? 1 : 0);
    outline.style.transform = `scale(${z})`; outline.style.borderWidth = `${3 / Math.sqrt(z)}px`;
    heroFrame(t);
    fx(sysEl).forEach((e, i) => { const k = out(win(t, 320 + Math.min(i, 12) * 34, 300)); setBand(e, Math.round(24 * (1 - k)), k); });
    if (t >= ZOOM_MS) fx(sysEl).forEach(e => { e.style.transform = ''; });
  }
  function zoomBoxes() {
    CX = path.length > 1 ? LEFT[0].x : 610;
    outline.style.borderColor = accOf(focused()); outline.style.boxShadow = `0 0 14px ${deepOf(focused())}`;
    [winEl, outline].forEach(e => { e.style.left = (CX - 90) + 'px'; }); faceEl.style.left = (CX - 89) + 'px';
  }
  function openSystem(s, dir) {
    zoomBoxes();
    if (dir > 0) { renderSystem(s); faceEl.style.backgroundImage = `url(${s.img || (s.img = cardImage(s))})`; }
    view = 'zoom';
    run(ZOOM_MS, t => zoomFrame(dir > 0 ? t : ZOOM_MS - t), () => {
      view = dir > 0 ? 'system' : 'browse';
      if (dir < 0) { sysEl.innerHTML = ''; crumb(); }
    });
  }

  // ---------- input ----------
  // A on a system card opens its page (the hub, with the game list one Select away).
  function pageContext(s, mode) {
    return { sys: s, mode, rect: { x: LEFT[0].x - 90, y: 158, w: 180, h: 252 }, faceUrl: faceSrc(s),
      fadeEls: [...fx(root.querySelector('#bbrowse')), carEl, crumbEl], onClosed: () => { view = 'browse'; } };
  }
  function openSysPage(s) {
    view = 'listzoom';
    GLIST.run(1, pageContext(s, 'hub'), typeof slow === 'number' ? slow : 1, () => { view = 'list'; });
  }
  function key(k) {
    if (view === 'list') { GLIST.key(k); return; }
    if (busy || view === 'listzoom') return;
    const A = k === 'Enter' || k === 'a', B = k === 'Escape' || k === 'b' || k === 'Backspace';
    if (view === 'system') {
      if (k === 'ArrowLeft' || k === 'ArrowRight') moveTile(k === 'ArrowRight' ? 1 : -1);
      else if (A) openList();
      else if (B) openSystem(curSys, -1);
      return;
    }
    const d = path.length - 1, n = here().children.length;
    if (k === 'ArrowLeft' || k === 'ArrowRight') slide(k === 'ArrowRight' ? 1 : -1);
    else if (A) {
      const f = focused();
      if (path.length === 1 && !f.children) { hooks.launch(f.name); return; }     // Settings, Arcade, Favourites
      if (f.children && f.children.length > 1) trick(1);
      else { curSys = f.children ? f.children[0] : f; sysRow = 0; openSysPage(curSys); }
    } else if (B) {
      if (path.length > 1) trick(-1);
    }
  }
  const hooks = { launch() {} };                            // the page decides what Settings, Arcade and Favourites do
  function open(where, at) {
    active = true; root.classList.add('on'); view = 'browse';
    path.length = 1; sel[0] = at != null ? at : 2;
    if (where !== 'launcher') path.push(TREE);
    if (where === 'nintendo') { sel[1] = 3; path.push(TREE.children[3]); }
    renderBrowse(); zoomBoxes();
    [...fx(root.querySelector('#bbrowse')), carEl, crumbEl].forEach(e => setBand(e, 0, 1));
    faceEl.style.opacity = 0; outline.style.opacity = 0; sysEl.innerHTML = '';
  }
  function close() { active = false; root.classList.remove('on'); }
  // Replay the trick from Consoles into the focused maker.
  function replay() {
    if (busy) return;
    if (view !== 'browse') return;
    open('launcher'); sel[1] = 0;
    setTimeout(() => trick(1), 350);
  }
  // Review: the SNES system view, or a single zoom frame.
  function review(name) {
    const at = name.startsWith('sysat-') ? +name.split('-')[1] : null;
    sel[2] = 2; open('nintendo');
    if (name === 'consoles') { open('consoles'); return; }
    if (name.startsWith('slide')) {                       // one frame of a step: slide-r-200, slide-l-120
      const [, dir, ms, maker] = name.split('-'); open('consoles');            // optional 4th part: open that maker first (0 = Atari)
      if (maker != null) { path.push(TREE.children[+maker]); sel[2] = 0; renderBrowse(); zoomBoxes(); }
      const keep = run; run = (d, f) => f(Math.min(+ms, d)); slideLeft(dir === 'l' ? -1 : 1); run = keep; return;
    }
    if (name.startsWith('back')) {                        // one frame of going back to the launcher: back-<ms>
      const ms = +name.split('-')[1]; open('consoles');
      const keep = run; run = (d, f) => f(Math.min(ms, TRICK_MS)); trick(-1); run = keep; return;
    }
    if (name.startsWith('glist')) {                         // one frame of a system page: glist-<ms>-<h|l>[-<root index>]
      const [, ms, m, idx] = name.split('-');
      if (idx != null) { open('launcher', +idx); path.push(ROOT.children[+idx]); sel[1] = 0; path.push(here().children[0]); sel[2] = 0; renderBrowse(); zoomBoxes(); }
      else open('nintendo');
      curSys = focused(); GLIST.preview(pageContext(curSys, m === 'l' ? 'list' : 'hub'), +ms); view = 'list'; return;
    }
    if (name.startsWith('sys-')) {                          // a collection's first system page: sys-3 Computers, sys-4 Handhelds
      const idx = +name.split('-')[1]; open('launcher', idx); path.push(ROOT.children[idx]); sel[1] = 0; path.push(here().children[0]); sel[2] = 0; renderBrowse(); zoomBoxes();
      curSys = focused(); renderSystem(curSys); faceEl.style.backgroundImage = `url(${cardImage(curSys)})`; view = 'system'; zoomFrame(ZOOM_MS); return;
    }
    if (name.startsWith('lvl-')) {                          // a collection's level: lvl-2 Consoles, lvl-3 Computers, lvl-4 Handhelds
      const idx = +name.split('-')[1]; open('launcher', idx); path.push(ROOT.children[idx]); sel[1] = 0; renderBrowse(); zoomBoxes(); return;
    }
    if (name.startsWith('trick')) {                         // trick-<ms> is one frame; an optional second number picks the collection (default 2)
      const nums = name.split('-').filter(x => /^\d+$/.test(x)).map(Number); sel[1] = 0; open('launcher', nums[1]);
      if (nums.length) { const keep = run; run = (d, f) => f(Math.min(nums[0], TRICK_MS)); trick(1); run = keep; } else setTimeout(() => trick(1), 500); return; }
    if (name === 'system' || at !== null) {
      curSys = focused(); renderSystem(curSys); faceEl.style.backgroundImage = `url(${cardImage(curSys)})`;
      view = at === null ? 'system' : 'zoom'; zoomFrame(at === null ? ZOOM_MS : at);
    }
  }
  async function init(stage) {
    build(stage);
    await Promise.all(['16px Nocive', '16px Xerxes', '48px Jersey', '72px Jersey'].map(f => document.fonts.load(f)).concat(
      ROOT.children.map(n => new Promise(r => { const im = new Image(); im.onload = () => { art[n.art] = im; r(); }; im.onerror = r; im.src = n.art; }))));
  }
  return { init, open, close, key, review, replay, hooks, TREE, ROOT, accOf, iconOf, deepOf, games, favs, GAMEPAD, ACC, ACC_DEEP, mix, get active() { return active; } };
})();
