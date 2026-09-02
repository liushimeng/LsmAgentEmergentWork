# Agent 工具定义
```json
{
"type": "function",
"function": {
"name": "read_file",
"description": "Read a file from the filesystem. Returns the contents prefixed with 1-based line numbers (`<n>\t<content>`). By default returns up to 300 lines; an output budget may return fewer. When a result shows a continuation offset, continue from that offset instead of rereading line 1. Use `offset` (1-based start line) and `limit` (max lines) when a larger relevant window is needed; avoid many tiny overlapping reads. To grab several disjoint windows at once (e.g. multiple symbols listed in a skeleton), pass `ranges` instead of paginating. If the path is a directory its entries are listed instead. Relative paths resolve against the working directory.",
"parameters": {
"type": "object",
"properties": {
"file_path": {
"type": "string",
"description": "Path to read (absolute, or relative to the working directory)"
}
,
"offset": {
"type": "integer",
"minimum": 1,
"description": "Start line, 1-based. Omit to start at line 1; after a partial result, use the next offset shown."
}
,
"limit": {
"type": "integer",
"minimum": 1,
"default": 300,
"description": "Maximum lines to read. Defaults to 300; the output byte budget may return fewer."
}
,
"ranges": {
"type": "array",
"description": "Read several disjoint line windows in ONE call instead of paginating. Each item is {offset, limit}. Takes precedence over top-level offset/limit and over the large-file skeleton. Prefer this when you already know the ranges (e.g. from a skeleton's per-symbol offset/limit). All windows share one output budget.",
"items": {
"type": "object",
"properties": {
"offset": {
"type": "integer",
"minimum": 1,
"description": "Start line, 1-based"
}
,
"limit": {
"type": "integer",
"minimum": 1,
"description": "Max lines for this window"
}
}
,
"required": [
"offset"
]
}
}
}
,
"required": [
"file_path"
]
}
}
}
```