# CodeStory discovery and results in code mode

## Inspect selected tool declarations

When the needed operation is known, inspect its exact callable declaration.
If you need an inventory first, print tool names, then inspect the declarations
for the operations you choose. All operations remain available.

In hosts exposing `ALL_TOOLS`, a names-only inventory avoids printing every
request and response schema:

```javascript
text(ALL_TOOLS.filter(tool => /codestory/i.test(tool.name)).map(tool => tool.name));
```

After choosing an operation, match its actual exposed name exactly. This example
selects search; choose any operation or set of operations the task needs:

```javascript
const selectedNames = new Set(["mcp__codestory__search"]);
text(ALL_TOOLS.filter(tool => selectedNames.has(tool.name)));
```

Keep each selected declaration complete. Inspect further declarations when they
become useful. A plugin-wide predicate over names and descriptions can print
many unused declarations; discovery does not require displaying all of them.

## Display each result payload once

Some hosts expose both `content` and `structuredContent` (or
`structured_content`) to JavaScript. Printing the whole wrapper can put the
same JSON payload into the model context twice.

Use one complete representation while retaining every other returned field.
This example removes only plain text blocks that exactly match the serialized structured
payload. Different spacing, key ordering or numeric spelling leaves the text
in place; parsing it first could discard precision that only the text retains. Distinct text, annotations, images,
resources, error flags and metadata stay intact. Keep the original result for
programmatic use.

After a CodeStory call returns `result`, the following works in a code-mode
cell with a `text` output helper:

```javascript
function withoutDuplicateJsonText(result) {
  const payload = result?.structuredContent ?? result?.structured_content;
  if (payload === undefined || !Array.isArray(result?.content)) return result;
  const serialized = JSON.stringify(payload);
  const content = result.content.filter(part => {
    if (part?.type !== 'text' || Object.keys(part).some(key => !['type', 'text'].includes(key))) return true;
    return part.text !== serialized;
  });
  if (content.length === result.content.length) return result;
  const displayed = { ...result };
  if (content.length) displayed.content = content;
  else delete displayed.content;
  return displayed;
}
text(withoutDuplicateJsonText(result));
```

This is a host display step. Tool requests, public response schemas, source
evidence and availability limits remain unchanged. A text-only response stays
as returned; do not replace it with an empty structured-content placeholder.
