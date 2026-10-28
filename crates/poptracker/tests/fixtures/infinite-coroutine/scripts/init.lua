-- Spins forever inside a coroutine: the instruction cap must cover coroutines,
-- not just the main thread.
coroutine.wrap(function()
  local x = 0
  while true do x = x + 1 end
end)()
