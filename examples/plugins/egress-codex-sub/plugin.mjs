import readline from "node:readline";

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });

rl.on("line", async (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;
  let req;
  try {
    req = JSON.parse(trimmed);
  } catch {
    return;
  }

  if (req.method === "discover_models") {
    process.stdout.write(
      JSON.stringify({
        jsonrpc: "2.0",
        id: req.id,
        result: [
          {
            id: "gpt-5.4",
            displayName: "GPT-5.4",
            vendorWireId: "gpt-5.4",
            family: "GPT",
            source: "live",
          },
          {
            id: "gpt-5.3-codex",
            displayName: "GPT-5.3-Codex",
            vendorWireId: "gpt-5.3-codex",
            family: "GPT",
            source: "live",
          },
        ],
      }) + "\n"
    );
    process.exit(0);
  }

  if (req.method === "invoke_stream") {
    const token = req.params?.credential?.payload?.accessToken ?? "";
    if (!token) {
      process.stdout.write(
        JSON.stringify({ event: "error", code: "AUTH_EXPIRED", message: "缺少 Codex accessToken" }) +
          "\n"
      );
      process.exit(1);
    }

    try {
      const resp = await fetch("https://chatgpt.com/backend-api/codex/responses", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
          Accept: "text/event-stream",
        },
        body: JSON.stringify(req.params.body),
      });
      process.stdout.write(
        JSON.stringify({
          event: "head",
          status: resp.status,
          headers: { "content-type": resp.headers.get("content-type") ?? "text/event-stream" },
        }) + "\n"
      );
      const reader = resp.body?.getReader();
      if (reader) {
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          process.stdout.write(
            JSON.stringify({
              event: "chunk",
              dataBase64: Buffer.from(value).toString("base64"),
            }) + "\n"
          );
        }
      }
      process.stdout.write(JSON.stringify({ event: "done" }) + "\n");
      process.exit(0);
    } catch (err) {
      process.stdout.write(
        JSON.stringify({ event: "error", code: "NETWORK_ERROR", message: String(err) }) + "\n"
      );
      process.exit(1);
    }
  }
});
