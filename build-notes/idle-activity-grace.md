# idle-activity-grace

## Goal

- Count slow but continuous computer use as active time.
- Prevent synced peers from multiplying active time when the same user is
  active on multiple computers.

## Decisions

- Keep the normal platform activity poll interval at 1 second.
- Treat normal activity samples as active while OS idle time is at most 10
  seconds.
- Keep the 10 second normal activity threshold internal for now.
- Later refinement removed the break-overlay idle check; normal activity still
  uses the 10 second grace threshold, but break overlays now count down by
  monotonic wall-clock deadlines.
- Keep the sync protocol unchanged. Runtime now interprets synced active-time
  events as remote activity signals sharing local wall-clock budget instead of
  unconditional extra scheduler time.

## Behavior

- A keypress every 2 seconds now keeps normal active time advancing every poll,
  so a 10 minute active-time break fires after about 10 wall-clock minutes.
- Local and synced active-time events consume the same wall-clock budget, so
  overlapping activity on two synced computers does not make a 10 minute break
  fire after 5 wall-clock minutes.
- Remote-only activity can still advance an idle peer's scheduler.
- Remote active-time events still reset combined idle tracking and are not
  rebroadcast.

## Commands

- `make check`

## Follow-up

- Manual multi-machine sync timing verification is pending.
