-- Supersedes the flat subscribed/unsubscribed daily cap from 002 with
-- tier-based monthly limits (Free / Pro / Max), matching the Stripe price
-- the user is actually on.
drop function if exists public.check_and_increment_usage(uuid, boolean, text, text, int, int);

alter table public.profiles
  add column if not exists plan text; -- 'pro' | 'max' | null (free / no active sub)

alter table public.profiles
  drop column if exists rewrite_day,
  drop column if exists rewrite_count_today;

create or replace function public.check_and_increment_usage(
  p_user_id uuid,
  p_month text,
  p_monthly_limit int
)
returns table(allowed boolean, monthly_count int)
language plpgsql
security definer
set search_path = public
as $$
declare
  v_month_count int;
begin
  -- Row lock serializes concurrent calls for the same user.
  perform 1 from public.profiles where id = p_user_id for update;

  select case when rewrite_month = p_month then rewrite_count else 0 end
  into v_month_count
  from public.profiles
  where id = p_user_id;

  if v_month_count >= p_monthly_limit then
    return query select false, v_month_count;
    return;
  end if;

  update public.profiles set
    rewrite_month = p_month,
    rewrite_count = v_month_count + 1
  where id = p_user_id;

  return query select true, v_month_count + 1;
end;
$$;

grant execute on function public.check_and_increment_usage(uuid, text, int) to service_role;
