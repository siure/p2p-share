const readline = require("node:readline");
const send = (event) => process.stdout.write(`${JSON.stringify(event)}\n`);
let active;
setTimeout(() => send({ kind: "worker_ready" }), 20);
readline.createInterface({ input: process.stdin }).on("line", (line) => {
  const command = JSON.parse(line);
  if (command.command === "shutdown") process.exit(0);
  if (command.command === "cancel") {
    if (active?.to === "ignore-cancel") return;
    send({ kind: "process_end", transfer_id: command.id, value: "canceled" });
    return;
  }
  active = command;
  if (command.to === "crash") process.exit(7);
  send({ kind: "status", transfer_id: command.id, value: String(process.pid) });
  // A late event must never finish the next transfer.
  send({ kind: "process_end", transfer_id: "previous-transfer", value: "completed" });
  if (command.to === "wait" || command.to === "ignore-cancel") return;
  send({ kind: "completed", transfer_id: command.id });
  send({ kind: "process_end", transfer_id: command.id, value: "completed" });
});
