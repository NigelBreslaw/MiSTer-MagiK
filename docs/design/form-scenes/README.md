# Particle Form scenes

These five 960x540 references define the commercial visual targets for the
MagiK Particle Form renderer. They were normalized from the approved concept
art and posterized independently to 5 red, 6 green, and 5 blue bits.

The references define composition, silhouette, hierarchy, depth, palette,
negative space, and motion intent. They are not pixel-matching targets.

| Demo | Scene | Budget | Entry | Hero | Exit |
|---:|---|---:|---:|---:|---:|
| 32 | Fractal Grid Terrain | 49,152 | 3 s | 15 s | 27 s |
| 33 | Layer-mapped Hologram | 40,960 | 3 s | 15 s | 27 s |
| 34 | Spherical Field Observatory | 32,768 | 3 s | 15 s | 27 s |
| 35 | Twisted Multi-form Cathedral | 65,536 | 3 s | 16 s | 27 s |
| 36 | Point-cloud Morph Passage | 24,576 | 4 s | 11 s | 26 s |

The initial renderer deliberately supports only order-independent RGB565
points, optional adjacent pixels, and bounded short topology. Glow, blur,
general alpha compositing, dynamic file inputs, audio reaction, and a runtime
node graph are outside this version.

See [acceptance.md](acceptance.md) for the qualification contract.
