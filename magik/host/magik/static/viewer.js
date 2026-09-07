const canvas = document.querySelector("#frame");
const context = canvas.getContext("2d");
const connection = document.querySelector("#connection");
async function tick() {
  try {
    const state = await fetch("/state").then(response => response.json());
    connection.className = state.error ? "error" : "";
    connection.textContent = state.error ? `Disconnected: ${state.error}` : (state.metrics ? "Receiving live probe data" : "Waiting for probe data…");
    document.querySelector("#state").textContent = JSON.stringify(state.metrics || {}, null, 2);
    document.querySelector("#logs").textContent = (state.logs || []).join("\n");
    if (state.frame && !state.error) {
      const response = await fetch("/frame");
      if (!response.ok) throw new Error("Preview unavailable");
      const bytes = await response.arrayBuffer();
      const header = new DataView(bytes);
      const width = header.getUint32(32, true), height = header.getUint32(36, true);
      const pixels = new Uint8Array(bytes, 72);
      canvas.width = width; canvas.height = height;
      const image = context.createImageData(width, height);
      for (let input = 0, output = 0; input < pixels.length; input += 2, output += 4) {
        const pixel = pixels[input] | pixels[input + 1] << 8;
        image.data[output] = (pixel >> 11) * 255 / 31;
        image.data[output + 1] = ((pixel >> 5) & 63) * 255 / 63;
        image.data[output + 2] = (pixel & 31) * 255 / 31;
        image.data[output + 3] = 255;
      }
      context.putImageData(image, 0, 0);
    }
  } catch (error) {
    connection.className = "error";
    connection.textContent = `Viewer error: ${error}`;
  }
  setTimeout(tick, 300);
}
tick();
