# Displaying CodeStory results in code mode

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
