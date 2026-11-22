-- M0 Wiki plugin: open a Content, edit it, save it through wiki-server.
-- The socket/server command are configurable so the M0 fake server can be
-- replaced by the real one later without changing the plugin.

local function server_cmd()
  local cmd = vim.g.wiki_server_cmd or { "wiki-server" }
  if type(cmd) == "string" then
    return { cmd }
  end
  return vim.list_extend({}, cmd)
end

local function socket()
  return vim.g.wiki_socket or "/tmp/harmion-wiki-m0.sock"
end

local function run(args, stdin)
  local cmd = server_cmd()
  vim.list_extend(cmd, args)
  vim.list_extend(cmd, { "--socket", socket() })
  local out = vim.fn.system(cmd, stdin)
  if vim.v.shell_error ~= 0 then
    error("wiki command failed: " .. table.concat(cmd, " ") .. "\n" .. out)
  end
  return vim.json.decode(out)
end

local function body_to_lines(body)
  local text = body:gsub("\n$", "")
  if text == "" then
    return { "" }
  end
  return vim.split(text, "\n", { plain = true })
end

local function lines_to_body(buf)
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  return table.concat(lines, "\n") .. "\n"
end

local function set_metadata(buf, content)
  vim.api.nvim_buf_set_var(buf, "wiki_id", content.id)
  vim.api.nvim_buf_set_var(buf, "wiki_rev", content.rev)
  vim.api.nvim_buf_set_var(buf, "wiki_type", content.type)
  if content.type == "snippet" and content.language then
    vim.bo[buf].filetype = content.language
  else
    vim.bo[buf].filetype = "markdown"
  end
end

local function open(id)
  local content = run({ "open", id })
  local buf = vim.api.nvim_create_buf(true, false)
  vim.bo[buf].buftype = "acwrite"
  vim.bo[buf].swapfile = false
  vim.api.nvim_buf_set_name(buf, "wiki://" .. content.id)
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, body_to_lines(content.body))
  set_metadata(buf, content)
  vim.bo[buf].modified = false
  vim.api.nvim_set_current_buf(buf)

  vim.api.nvim_create_autocmd("BufWriteCmd", {
    buffer = buf,
    callback = function()
      local body = lines_to_body(buf)
      local saved = run({ "save", content.id, "--rev", tostring(vim.b[buf].wiki_rev) }, body)
      vim.api.nvim_buf_set_var(buf, "wiki_rev", saved.rev)
      vim.bo[buf].modified = false
      vim.notify(("wiki: saved %s (rev %d)"):format(saved.title, saved.rev), vim.log.levels.INFO)
    end,
  })
  vim.notify(("wiki: opened %s"):format(content.title), vim.log.levels.INFO)
end

local function list()
  local data = run({ "list" })
  local items = {}
  for _, item in ipairs(data.items) do
    table.insert(items, {
      filename = "wiki://" .. item.id,
      lnum = 1,
      text = ("[%s] %s"):format(item.type, item.title),
    })
  end
  vim.fn.setqflist(items)
  vim.cmd("copen")
end

vim.api.nvim_create_user_command("WikiOpen", function(args)
  if args.args == "" then
    local id = vim.b.wiki_id
    if not id then
      error("WikiOpen: content id required")
    end
    open(id)
    return
  end
  open(args.args)
end, { nargs = "?" })

vim.api.nvim_create_user_command("WikiList", list, {})
vim.api.nvim_create_user_command("WikiSave", function()
  vim.cmd("write")
end, {})
