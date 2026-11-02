Tracker:AddItems("items.json")
Tracker:AddLocations("locations.json")
local on = function(code) Tracker:FindObjectForCode(code).Active = true end

assert(Highlight.Avoid == -1 and Highlight.None == 0 and Highlight.NoPriority == 1
  and Highlight.Unspecified == 2 and Highlight.Priority == 3)

Archipelago:AddClearHandler("clear", function()
  Archipelago:Get({"got", "nothing_there"})
  Archipelago:SetNotify({"watched", "both", "notify_missing"})
  Archipelago:Get({"both"})
  assert(Tracker:FindObjectForCode("@Place/B").Highlight == Highlight.None, "starts at none")
  Archipelago:Get("not a table")
  if #Archipelago.CheckedLocations == 1 and Archipelago.CheckedLocations[1] == 2
    and #Archipelago.MissingLocations == 2 then
    on("locations")
  end
  Tracker:FindObjectForCode("@Place/A").Highlight = Highlight.Priority
end)

Archipelago:AddItemHandler("item", function(_, _, _, sender)
  if sender == 7 then on("sender") end
end)

Archipelago:AddRetrievedHandler("got", function(key, value)
  if key == "got" and value.n == 4 then
    on("retrieved")
    Archipelago:Get({"second"})
  elseif key == "nothing_there" and value == nil then
    on("missing_nil")
  elseif key == "second" and value == "asked by a handler" then
    on("chained")
  end
end)

Archipelago:AddSetReplyHandler("watched", function(key, value, old)
  if key == "watched" and value == 5 and old == nil then on("notified") end
  if key == "both" or key == "notify_missing" then error("told twice, or of nothing") end
end)
