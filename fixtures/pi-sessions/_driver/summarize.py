import json,sys
for line in open(sys.argv[1]):
    d=json.loads(line); b=d['body'] or {}
    msgs=[]
    for m in b.get('messages',[]):
        if m['role']=='system': continue
        c=m.get('content')
        if isinstance(c,list): c=''.join(p.get('text','') for p in c)
        s=f"{m['role']}:{(c or '')[:70]!r}"
        if m.get('tool_calls'): s+=" calls="+",".join(t['function']['name'] for t in m['tool_calls'])
        msgs.append(s)
    extra={k:v for k,v in b.items() if k not in('messages','tools','stream','stream_options','store')}
    print(d['n'], extra, ' | '.join(msgs))
