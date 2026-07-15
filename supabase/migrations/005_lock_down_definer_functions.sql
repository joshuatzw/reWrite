-- Lock down SECURITY DEFINER functions to service_role only.
--
-- SECURITY DEFINER functions run with the definer's privileges and bypass RLS.
-- Postgres grants EXECUTE to PUBLIC by default, so despite the explicit
-- `grant execute ... to service_role` in earlier migrations, the anon and
-- authenticated roles could still call these directly via PostgREST rpc.
--
-- For check_and_increment_usage this was a real free-tier bypass: a signed-in
-- user could call it with a bogus p_month (e.g. '2099-01') to overwrite their
-- profiles.rewrite_month. The rewrite edge function then reads their
-- current-month usage as 0 on every call and hands out unlimited rewrites,
-- billed to our Anthropic key. The same arbitrary-p_user_id path also let a
-- caller inflate other users' counters.
--
-- Revoking PUBLIC (and anon/authenticated explicitly, belt-and-suspenders)
-- closes the direct-call door. The edge function is unaffected: it calls these
-- with the service_role key. Trigger and event-trigger functions keep firing
-- normally -- the trigger mechanism does not check EXECUTE on the function.

revoke execute on function public.check_and_increment_usage(uuid, text, integer)
  from public, anon, authenticated;

revoke execute on function public.handle_new_user()
  from public, anon, authenticated;

revoke execute on function public.rls_auto_enable()
  from public, anon, authenticated;
