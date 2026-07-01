import { serve } from "https://deno.land/std@0.208.0/http/server.ts";

const html = `<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>Checkout cancelled</title>
    <style>
      body { font-family: system-ui, sans-serif; display: flex; align-items: center; justify-content: center; height: 100vh; margin: 0; background: #16161a; color: #fff; }
    </style>
  </head>
  <body>
    <p>No worries &mdash; you can upgrade anytime from reWrite.</p>
    <script>window.location.href = "rewrite://checkout-cancelled";</script>
  </body>
</html>`;

serve(() => new Response(html, { headers: { "Content-Type": "text/html; charset=utf-8" } }));
