-- Removes the deprecated "Writing Style / Tone of Voice" feature.
--
-- The tone_of_voice table, its RLS policies, and the set_default_tone RPC were
-- created directly in the dashboard and never captured as a migration, so they
-- lingered in the live database after the app-side feature was removed. This
-- migration deletes them so the schema matches the app (which no longer
-- references tone_of_voice anywhere).
--
-- Destructive: any stored user-authored tones are permanently deleted. That is
-- intended — the feature is retired.

-- Drop every overload of set_default_tone regardless of its argument signature,
-- so we don't have to hardcode the exact parameter types.
do $$
declare
  fn record;
begin
  for fn in
    select oid::regprocedure as sig
    from pg_proc
    where proname = 'set_default_tone'
      and pronamespace = 'public'::regnamespace
  loop
    execute 'drop function ' || fn.sig;
  end loop;
end $$;

-- Cascade drops the table's RLS policies, indexes, triggers, and grants.
drop table if exists public.tone_of_voice cascade;
