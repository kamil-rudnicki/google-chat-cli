# How to

## How to find all threads that contain specific message?

```bash
gchat search invoice --all --max 5000 \
  | jq -r '.data.results[] | (.thread.name // .message.thread.name // empty)' \
  | sort -u
```

## How to get all unread messages?

```bash
gchat search unread --all --max 5000
```
