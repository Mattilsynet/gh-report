import init, { tick } from "./pkg/gh_report_queue_viz.js";

async function main() {
  await init();
  setInterval(() => {
    tick();
  }, 80);
}

main();
