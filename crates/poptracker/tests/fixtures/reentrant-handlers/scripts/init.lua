-- Handlers that register more handlers, or load tracker data, while they run.
-- Both take the engine's mutable borrow, so the host must not still hold its
-- borrow of the handler list while it calls into a handler (issue #2). Each
-- late handler marks its item, so the test can see that it ran.
Tracker:AddItems("items/items.json")

local function mark(code)
  return function() Tracker:FindObjectForCode(code).Active = true end
end

Archipelago:AddClearHandler("clear", function(slot_data)
  Archipelago:AddItemHandler("late item", mark("late_item"))
  Tracker:AddItems("items/more.json")
end)
Archipelago:AddItemHandler("item", function(index, item_id, item_name, player)
  Archipelago:AddLocationHandler("late location", mark("late_location"))
end)
Archipelago:AddLocationHandler("location", function(location_id, location_name)
  Archipelago:AddClearHandler("late clear", mark("late_clear"))
end)
Archipelago:AddRetrievedHandler("retrieved", function(key, value)
  Archipelago:AddSetReplyHandler("late set reply", mark("late_set_reply"))
end)
Archipelago:AddSetReplyHandler("set reply", function(key, value)
  Archipelago:AddRetrievedHandler("late retrieved", mark("late_retrieved"))
end)
