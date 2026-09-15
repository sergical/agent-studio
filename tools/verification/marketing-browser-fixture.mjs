import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
const [endpoint, root, output] = process.argv.slice(2);
const socket = new WebSocket(endpoint);
let sequence = 0;
const pending = new Map();
const captures = [];
const assets = [];
const blocked = [];
function persist() {
  fs.writeFileSync(output, JSON.stringify({ captures, assets, blocked }, null, 2));
}
function command(method, params = {}, sessionId) {
  const id = ++sequence;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params, sessionId }));
  });
}
const mime = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".mp4": "video/mp4",
  ".svg": "image/svg+xml",
};
socket.onmessage = async ({ data }) => {
  const message = JSON.parse(data);
  if (message.id) {
    const callback = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) callback?.reject(new Error(JSON.stringify(message.error)));
    else callback?.resolve(message.result);
    return;
  }
  if (message.method !== "Fetch.requestPaused") return;
  const { requestId, request } = message.params;
  const sessionId = message.sessionId;
  const url = new URL(request.url);
  try {
    if (url.hostname === "example.invalid") {
      captures.push({
        method: request.method,
        url: request.url,
        body: request.postData ?? "",
        headers: request.headers,
      });
      persist();
      await command(
        "Fetch.fulfillRequest",
        {
          requestId,
          responseCode: 200,
          responseHeaders: [
            { name: "Content-Type", value: "application/json" },
            { name: "Access-Control-Allow-Origin", value: "*" },
            { name: "Access-Control-Allow-Headers", value: "*" },
          ],
          body: Buffer.from("{}").toString("base64"),
        },
        sessionId,
      );
      return;
    }
    const pathname = url.pathname === "/" ? "/index.html" : decodeURIComponent(url.pathname);
    const file = path.resolve(root, `.${pathname}`);
    if (
      url.hostname !== "marketing-fixture.invalid" ||
      !file.startsWith(root + path.sep) ||
      !fs.existsSync(file) ||
      !fs.statSync(file).isFile()
    ) {
      blocked.push({ url: request.url });
      persist();
      await command("Fetch.failRequest", { requestId, errorReason: "BlockedByClient" }, sessionId);
      return;
    }
    const full = fs.readFileSync(file);
    let body = full;
    let responseCode = 200;
    const headers = [
      { name: "Content-Type", value: mime[path.extname(file)] ?? "application/octet-stream" },
      { name: "Cache-Control", value: "no-store" },
    ];
    const range = request.headers.Range ?? request.headers.range;
    const match = range?.match(/^bytes=(\d+)-(\d*)$/);
    if (match) {
      const start = Number(match[1]);
      const end = match[2] ? Math.min(Number(match[2]), full.length - 1) : full.length - 1;
      body = full.subarray(start, end + 1);
      responseCode = 206;
      headers.push({ name: "Content-Range", value: `bytes ${start}-${end}/${full.length}` });
    }
    assets.push({
      path: pathname,
      bytes: body.length,
      sha256: crypto.createHash("sha256").update(full).digest("hex"),
    });
    persist();
    await command(
      "Fetch.fulfillRequest",
      { requestId, responseCode, responseHeaders: headers, body: body.toString("base64") },
      sessionId,
    );
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
  }
};
await new Promise((resolve, reject) => {
  socket.onopen = resolve;
  socket.onerror = reject;
});
const { targetInfos } = await command("Target.getTargets");
const target = targetInfos.find((item) => item.type === "page" && item.url === "about:blank");
if (!target) throw new Error("Expected an isolated blank page");
const { sessionId } = await command("Target.attachToTarget", {
  targetId: target.targetId,
  flatten: true,
});
await command("Fetch.enable", { patterns: [{ urlPattern: "*" }] }, sessionId);
process.stdout.write("FIXTURE_READY\n");
process.on("SIGTERM", async () => {
  await command("Fetch.disable", {}, sessionId);
  socket.close();
});
