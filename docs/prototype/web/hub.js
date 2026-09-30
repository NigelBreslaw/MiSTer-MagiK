// The hub half of a system page: identity (maker / year / generation), the
// name, the game count and the Games / Recent / Favourites tiles, laid out to
// fit the left side of the page while the device stays on the right. Shared by
// the system pages and the Arcade page.
window.HUB = (() => {
  const hex = h => [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16));
  const mix = (a, b, k) => { const x = hex(a), y = hex(b); return `rgb(${x.map((v, i) => Math.round(v + (y[i] - v) * k)).join(',')})`; };
  const st = document.createElement('style');
  st.textContent = `
    .hub{position:absolute;inset:0;pointer-events:none}
    .hub .hn{position:absolute;font-family:Jersey;color:var(--cream);white-space:normal}
    .hub .htile{position:absolute;top:318px;width:140px;height:128px;border-radius:10px;box-sizing:border-box;border:2px solid var(--hb);
      background:linear-gradient(180deg,var(--hg1),var(--hg2));transition:transform .16s cubic-bezier(.2,.8,.2,1),box-shadow .16s,border-color .16s}
    .hub .htile .num{position:absolute;left:14px;top:12px;font-family:Jersey;font-size:52px;line-height:1;color:var(--cream)}
    .hub .htile .lbl{position:absolute;left:14px;bottom:14px;color:var(--muted)}
    .hub .htile.empty .num{color:var(--muted)}
    .hub .htile.on{border-color:var(--ha);box-shadow:0 0 20px var(--hd),0 14px 26px #000;transform:translateY(-8px)}
    .hub .htile.on .lbl{color:var(--cream)}
    .hub .htile.on::after{content:"";position:absolute;left:14px;right:14px;bottom:0;height:3px;background:var(--ha)}`;
  document.head.append(st);

  // Height of the name at a size, wrapped to the panel width.
  function measure(text, size, lh, width) {
    const m = document.createElement('div');
    m.style.cssText = `position:fixed;left:-9999px;top:0;visibility:hidden;width:${width}px;font-family:Jersey;font-size:${size}px;line-height:${lh}px;white-space:normal`;
    m.textContent = text; document.body.append(m); const h = m.offsetHeight; m.remove(); return h;
  }

  // cfg: { title, subtitle, count, tiles: [{ n, label }], captions: [string], accent }
  function build(cfg) {
    const el = document.createElement('div'); el.className = 'hub';
    const A = cfg.accent;
    el.style.setProperty('--ha', A); el.style.setProperty('--hb', mix('#000000', A, .45));
    el.style.setProperty('--hg1', mix('#0c161e', A, .24)); el.style.setProperty('--hg2', mix('#05080c', A, .06));
    el.style.setProperty('--hd', mix('#000000', A, .6));
    const size = cfg.title.length > 13 ? 48 : 64, lh = Math.round(size * .9), h = measure(cfg.title, size, lh, 440);
    const bands = [];
    const add = html => { const t = document.createElement('div'); t.innerHTML = html; const n = t.firstElementChild; el.append(n); bands.push(n); return n; };
    add(`<div class="a m muted" style="left:30px;top:104px">${cfg.subtitle}</div>`);
    add(`<div class="hn" style="left:28px;top:126px;width:440px;font-size:${size}px;line-height:${lh}px">${cfg.title}</div>`);
    add(`<div class="a m" style="left:30px;top:${126 + h + 10}px">${cfg.count}</div>`);
    const tiles = cfg.tiles.map((t, i) => add(`<div class="htile${t.n ? '' : ' empty'}" style="left:${30 + i * 150}px">
      <span class="num">${t.n}</span><span class="lbl h">${t.label}</span></div>`));
    const cap = add(`<div class="a m" style="left:30px;top:470px;color:#c9c3b3"></div>`);
    let focus = -1;
    function setFocus(i, animate = true) {
      if (i === focus) return; focus = i;
      tiles.forEach((t, k) => t.classList.toggle('on', k === i));
      cap.textContent = cfg.captions[i] || '';
      if (animate) cap.animate([{ opacity: 0, transform: 'translateX(10px)' }, { opacity: 1, transform: 'none' }], { duration: 180, easing: 'cubic-bezier(.2,.8,.2,1)' });
    }
    return { el, bands, tiles, setFocus, get focus() { return focus; } };
  }
  return { build, mix };
})();
