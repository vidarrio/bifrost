Tracker:AddItems("items/items.json")
Archipelago:AddClearHandler("clear", function(slot_data)
  local opts = slot_data.Data and slot_data.Data.Options
  Tracker:FindObjectForCode("from_slot_data").Active = opts ~= nil and opts.Flag == true
  Tracker:FindObjectForCode("players_known").Active = Archipelago.Seed == "seed1"
    and Archipelago:GetPlayerGame(2) == "Game B"
    and Archipelago:GetPlayerAlias(2) == "Bo"
    and Archipelago:GetPlayerGame(3) == nil
    and Archipelago:GetPlayerAlias(3) == ""
    and Archipelago.GetPlayerGame() == nil
    and Archipelago.GetPlayerAlias() == ""
    and Archipelago.GetPlayerGame(2) == "Game B"
end)
