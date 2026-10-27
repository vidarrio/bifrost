-- Handlers that register more handlers while they run. Registering takes the
-- engine's mutable borrow, so the host must not still hold its borrow of the
-- handler list while it calls into a handler (issue #2).
Archipelago:AddClearHandler("clear", function(slot_data)
  Archipelago:AddItemHandler("late item", function() end)
end)
Archipelago:AddItemHandler("item", function(index, item_id, item_name, player)
  Archipelago:AddLocationHandler("late location", function() end)
end)
Archipelago:AddLocationHandler("location", function(location_id, location_name)
  Archipelago:AddClearHandler("late clear", function() end)
end)
Archipelago:AddRetrievedHandler("retrieved", function(key, value)
  Archipelago:AddSetReplyHandler("late set reply", function() end)
end)
Archipelago:AddSetReplyHandler("set reply", function(key, value)
  Archipelago:AddRetrievedHandler("late retrieved", function() end)
end)
