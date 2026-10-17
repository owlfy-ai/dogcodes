import json,sys
x=json.load(sys.stdin);t=x.get("toolCall",{});a=t.get("args",{})
if isinstance(a,str):
 try:a=json.loads(a)
 except ValueError:a={}
allow=t.get("name")=="call_mcp_tool" and a.get("ServerName")=="dogcodes_history_probe" and a.get("ToolName")=="searchHistory"
with open(sys.argv[1],"a") as f:f.write(json.dumps({"tool":t,"allowed":allow})+"\n")
print(json.dumps({"decision":"allow" if allow else "deny","reason":"Only fixed read-only history fixture permitted"}))
