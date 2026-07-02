-- Daily usage counter (separate from the monthly free-tier counter) so that
-- subscribed ("unlimited") accounts still have a sane abuse/cost ceiling.
alter table public.profiles
  add column if not exists rewrite_day text,
  add column if not exists rewrite_count_today int not null default 0;

-- Atomically checks the caller's quota and, if allowed, records the usage —
-- closes the read-then-write race where concurrent requests could each read
-- a stale count and all pass the limit check.
create or replace function public.check_and_increment_usage(
  p_user_id uuid,
  p_is_subscribed boolean,
  p_month text,
  p_day text,
  p_monthly_limit int,
  p_daily_limit int
)
returns table(allowed boolean, monthly_count int, daily_count int)
language plpgsql
security definer
set search_path = public
as $$
declare
  v_month_count int;
  v_day_count int;
begin
  -- Row lock serializes concurrent calls for the same user.
  perform 1 from public.profiles where id = p_user_id for update;

  select
    case when rewrite_month = p_month then rewrite_count else 0 end,
    case when rewrite_day = p_day then rewrite_count_today else 0 end
  into v_month_count, v_day_count
  from public.profiles
  where id = p_user_id;

  if not p_is_subscribed and v_month_count >= p_monthly_limit then
    return query select false, v_month_count, v_day_count;
    return;
  end if;

  if p_is_subscribed and v_day_count >= p_daily_limit then
    return query select false, v_month_count, v_day_count;
    return;
  end if;

  update public.profiles set
    rewrite_month = p_month,
    rewrite_count = v_month_count + 1,
    rewrite_day = p_day,
    rewrite_count_today = v_day_count + 1
  where id = p_user_id;

  return query select true, v_month_count + 1, v_day_count + 1;
end;
$$;

grant execute on function public.check_and_increment_usage(uuid, boolean, text, text, int, int) to service_role;
