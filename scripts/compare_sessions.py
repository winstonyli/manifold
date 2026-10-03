import json, sys
a, b = (json.load(open(p))["nodes"] for p in sys.argv[1:3])
key = lambda nodes: sorted(json.dumps(n, sort_keys=True) for n in nodes)
if key(a) != key(b):
    print("FAIL: sessions differ", sys.argv[1:3]); sys.exit(1)
print("ok:", len(a), "nodes")
