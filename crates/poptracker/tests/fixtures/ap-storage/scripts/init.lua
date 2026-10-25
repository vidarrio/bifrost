Tracker:AddItems("items/items.json")
Tracker:AddLocations("locations/locations.json")
ScriptHost:LoadScript("scripts/logic.lua")
if Archipelago then
    ScriptHost:LoadScript("scripts/autotracking.lua")
end
