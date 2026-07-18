import init, { tick } from "./pkg/cherry_pit_sd_viz.js";

async function main() {
  await init();
  setInterval(() => {
    tick();
  }, 320);
}

main();
