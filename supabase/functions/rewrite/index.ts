import { serve } from "https://deno.land/std@0.208.0/http/server.ts";

const ANTHROPIC_API_URL = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_API_VERSION = "2023-06-01";

interface RewriteRequest {
  system_prompt: string;
  user_message: string;
  model: string;
}

serve(async (req) => {
  if (req.method === "OPTIONS") {
    return new Response(null, {
      headers: {
        "Access-Control-Allow-Origin": "*",
        "Access-Control-Allow-Headers": "authorization, content-type",
        "Access-Control-Allow-Methods": "POST, OPTIONS",
      },
    });
  }

  if (req.method !== "POST") {
    return new Response(JSON.stringify({ error: "Method not allowed" }), {
      status: 405,
      headers: { "Content-Type": "application/json" },
    });
  }

  const apiKey = Deno.env.get("ANTHROPIC_API_KEY");
  if (!apiKey) {
    return new Response(JSON.stringify({ error: "Service not configured" }), {
      status: 500,
      headers: { "Content-Type": "application/json" },
    });
  }

  let body: RewriteRequest;
  try {
    body = await req.json();
  } catch {
    return new Response(JSON.stringify({ error: "Invalid JSON body" }), {
      status: 400,
      headers: { "Content-Type": "application/json" },
    });
  }

  const { system_prompt, user_message, model } = body;
  if (!system_prompt || !user_message || !model) {
    return new Response(
      JSON.stringify({ error: "Missing required fields: system_prompt, user_message, model" }),
      { status: 400, headers: { "Content-Type": "application/json" } }
    );
  }

  const anthropicRes = await fetch(ANTHROPIC_API_URL, {
    method: "POST",
    headers: {
      "x-api-key": apiKey,
      "anthropic-version": ANTHROPIC_API_VERSION,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      model,
      max_tokens: 4096,
      system: system_prompt,
      messages: [{ role: "user", content: user_message }],
    }),
  });

  if (!anthropicRes.ok) {
    const detail = await anthropicRes.text();
    return new Response(
      JSON.stringify({ error: `API error ${anthropicRes.status}: ${detail}` }),
      { status: anthropicRes.status, headers: { "Content-Type": "application/json" } }
    );
  }

  const data = await anthropicRes.json();

  if (data.stop_reason === "max_tokens") {
    return new Response(
      JSON.stringify({ error: "Response was cut off — the text may be too long to rewrite in one pass." }),
      { status: 422, headers: { "Content-Type": "application/json" } }
    );
  }

  const text = data.content
    ?.find((c: { type: string; text?: string }) => c.type === "text")?.text;

  if (!text) {
    return new Response(
      JSON.stringify({ error: "No text in API response" }),
      { status: 500, headers: { "Content-Type": "application/json" } }
    );
  }

  return new Response(JSON.stringify({ text }), {
    headers: { "Content-Type": "application/json" },
  });
});
