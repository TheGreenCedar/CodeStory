# Optional source-artifact handoff

Optionally save a chosen tool result for a fresh native helper without printing
its source body first. Choose your own operation, paths and whether to delegate;
ordinary source tools can use this too. Every MCP call still needs absolute
`project`. No preliminary packet, mandatory helper or new permission is implied.

Save the complete original MCP JSON-data wrapper before display filtering:
content, structured data, errors, annotations, `_meta`, publication/source hashes,
freshness, truncation and gaps remain intact. Do not apply this example to
arbitrary JavaScript objects. Artifact bytes are not wire bytes or model tokens.

This POSIX example needs existing `python3` and owned permitted scratch outside
source. Fresh private0700 directory ownership prevents prior-file overwrite;
`apply_patch` itself is not create-new. Raw body enters direct patch data, never
shell arguments. Exact readback stays in JavaScript; print only the manifest.
A capped/failed verification falls back to the existing complete-display flow.

After the selected call returns `result`, use the actual declared nested tools:

```javascript
// POSIX + existing python3. JSON-data MCP results only; no arbitrary JS objects.
function quote(s) { return "'" + s.replace(/'/g, "'\\''") + "'"; }
function completeJson(result) {
  const json=JSON.stringify(result, (_, v) => {
    if (v === undefined || typeof v === 'bigint') throw Error('non-JSON value');
    if (typeof v === 'number' && (!Number.isFinite(v) || Object.is(v,-0)
        || (Number.isInteger(v) && !Number.isSafeInteger(v)))) throw Error('unsupported number');
    return v;
  }); // Native stringify rejects cycles. No filtering of the returned envelope.
  if (typeof json !== 'string') throw Error('non-JSON result');
  return json;
}
async function saveResultArtifact(result, {project, scratch}) {
  const json=completeJson(result);
  const prep="import pathlib,tempfile,sys; p=pathlib.Path(sys.argv[1]); s=pathlib.Path(sys.argv[2]); assert p.is_absolute() and s.is_absolute(); p=p.resolve(strict=True); s=s.resolve(strict=True); assert p.is_dir() and s.is_dir() and s!=p and p not in s.parents; print(tempfile.mkdtemp(prefix='source-handoff-',dir=s))";
  const made=await tools.exec_command({cmd:'python3 -c '+quote(prep)+' '+quote(project)+' '+quote(scratch),max_output_tokens:1000});
  if (made.exit_code!==0 || made.session_id) throw Error('scratch unavailable');
  const directory=made.output.trim();
  if (!directory || /[\r\n]/.test(directory)) throw Error('invalid scratch path');
  const path=directory+'/result.json'; // Sole writer in a fresh private0700 directory.
  await tools.apply_patch('*** Begin Patch\n*** Add File: '+path+'\n+'+json+'\n*** End Patch');
  const verify="import pathlib,hashlib,json,sys; p=pathlib.Path(sys.argv[1]); assert p.is_file() and not p.is_symlink(); b=p.read_bytes(); json.loads(b); print(json.dumps({'path':str(p),'bytes':len(b),'sha256':hashlib.sha256(b).hexdigest(),'readback':b.decode('utf-8')}))";
  const checked=await tools.exec_command({cmd:'python3 -c '+quote(verify)+' '+quote(path),max_output_tokens:Math.max(10000,json.length*2)});
  if (checked.exit_code!==0 || checked.session_id) throw Error('verification unavailable');
  const manifest=JSON.parse(checked.output); // Readback remains only in JavaScript.
  if (manifest.readback!==json+'\n') throw Error('readback mismatch');
  delete manifest.readback;
  return {...manifest,project};
}
// project/scratch are already authorized; result is the primary's chosen tool result.
try { text(await saveResultArtifact(result,{project,scratch})); }
catch { text({artifact_handoff:'unavailable'}); text(result); }
```

If you delegate, call native collaboration **outside code-mode**: use a fresh
helper without inherited history under the authorized model/policy. Pass the
verified path/hash, task and unresolved question; request source-backed findings
with project identity, file/range anchors, omissions and validation status.
Charge all primary/helper tokens, raw-source reading, failures and recovery.
Keep ordinary reads and required validation available; summaries and graph or
packet availability do not establish sufficiency. The primary can verify
originals and resolve gaps normally. If any primitive/helper is unavailable,
use an equivalent verified writer or ordinary flow; do not install dependencies.
