// A stub model provider for the Pi tests (tests/pi.rs; answer cc34f6275877 (3)): an HTTPS server on
// 127.0.0.1 speaking just enough of Anthropic's streaming Messages API, and of OpenAI's streaming chat
// completions as OpenRouter serves them, for one Pi turn, so a turn runs end to end with no model. Run
// by the test prefix's own node, outside the sandbox:
//
//   node pi-stub.mjs <cert.pem> <key.pem> <log.jsonl> <token>
//
// It prints its port, then answers every POST .../v1/messages and .../chat/completions with one
// assistant message of fixed text and the same usage (input 1200, cache read 4000, cache write 300,
// output 7), and logs each request as a JSON line: method, path, whether its bearer token was <token>
// (never the value), the model asked for and the names of the tools declared. Anything else gets 404.
// The certificate and key are made by the test, at test time; none is kept in the repository.

import { appendFileSync, readFileSync } from "node:fs";
import { createServer } from "node:https";

const [cert, key, log, token] = process.argv.slice(2);

const sse = (res, event, data) => res.write(`event: ${event}\ndata: ${JSON.stringify({ type: event, ...data })}\n\n`);

const server = createServer({ cert: readFileSync(cert), key: readFileSync(key) }, (req, res) => {
  let body = "";
  req.on("data", (chunk) => (body += chunk));
  req.on("end", () => {
    let request = {};
    try {
      request = JSON.parse(body || "{}");
    } catch {}
    const auth = req.headers.authorization;
    appendFileSync(
      log,
      JSON.stringify({
        method: req.method,
        path: req.url,
        auth: auth === `Bearer ${token}` ? "the token" : auth ? "another" : "none",
        model: request.model ?? null,
        tools: (request.tools ?? []).map((t) => t.name ?? t.function?.name),
      }) + "\n",
    );
    const path = new URL(req.url, "https://stub").pathname;
    if (req.method === "POST" && path.endsWith("/chat/completions")) {
      res.writeHead(200, { "content-type": "text/event-stream" });
      const chunk = (rest) =>
        res.write(`data: ${JSON.stringify({ id: "gen-stub-1", object: "chat.completion.chunk", model: request.model, ...rest })}\n\n`);
      chunk({ choices: [{ index: 0, delta: { role: "assistant", content: "Done." }, finish_reason: null }] });
      chunk({ choices: [{ index: 0, delta: {}, finish_reason: "stop" }] });
      const details = { cached_tokens: 4000, cache_write_tokens: 300 };
      chunk({ choices: [], usage: { prompt_tokens: 5500, completion_tokens: 7, prompt_tokens_details: details } });
      res.write("data: [DONE]\n\n");
      res.end();
      return;
    }
    if (req.method !== "POST" || path !== "/v1/messages") {
      res.writeHead(404).end();
      return;
    }
    res.writeHead(200, { "content-type": "text/event-stream" });
    const usage = { input_tokens: 1200, cache_creation_input_tokens: 300, cache_read_input_tokens: 4000, output_tokens: 1 };
    sse(res, "message_start", {
      message: { id: "msg_stub_1", type: "message", role: "assistant", model: request.model, content: [], stop_reason: null, stop_sequence: null, usage },
    });
    sse(res, "content_block_start", { index: 0, content_block: { type: "text", text: "" } });
    sse(res, "content_block_delta", { index: 0, delta: { type: "text_delta", text: "Done." } });
    sse(res, "content_block_stop", { index: 0 });
    sse(res, "message_delta", { delta: { stop_reason: "end_turn", stop_sequence: null }, usage: { output_tokens: 7 } });
    sse(res, "message_stop", {});
    res.end();
  });
});

server.listen(0, "127.0.0.1", () => console.log(server.address().port));
