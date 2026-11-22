-- Wiki Neovim integration.
-- The server is the only component that holds Harmion credentials. Neovim
-- talks to it over a Unix socket and uses wiki://<id> virtual buffers.

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

local function lsp_start()
  if vim.g.wiki_lsp == false then
    return
  end
  local cmd = vim.g.wiki_lsp_cmd or { "wiki-lsp" }
  local ok, client = pcall(vim.lsp.start, {
    name = "wiki",
    cmd = cmd,
    root_dir = vim.fn.getcwd(),
    init_options = { socket = socket() },
  })
  if not ok then
    vim.notify("wiki: LSP start failed: " .. tostring(client), vim.log.levels.DEBUG)
  end
end

local autosave_timers = {}

local function cancel_autosave(buf)
  local timer = autosave_timers[buf]
  if timer then
    timer:stop()
    timer:close()
    autosave_timers[buf] = nil
  end
end

local function schedule_autosave(buf)
  if vim.g.wiki_autosave == false or not vim.api.nvim_buf_is_valid(buf) then
    return
  end
  cancel_autosave(buf)
  local timer = vim.uv.new_timer()
  autosave_timers[buf] = timer
  timer:start(1500, 0, function()
    vim.schedule(function()
      autosave_timers[buf] = nil
      if vim.api.nvim_buf_is_valid(buf) and vim.bo[buf].modified then
        vim.api.nvim_buf_call(buf, function()
          vim.cmd("silent write")
        end)
      end
    end)
  end)
end

local function resolve_id(value)
  local opened = pcall(run, { "open", value })
  if opened then
    return value
  end
  local data = run({ "search", value })
  for _, item in ipairs(data.items or {}) do
    if item.title == value then
      return item.id
    end
  end
  if data.items and data.items[1] then
    return data.items[1].id
  end
  error("WikiOpen: no Content matches " .. value)
end

local function open(id_or_title)
  local id = resolve_id(id_or_title)
  local content = run({ "open", id })
  local buf = vim.api.nvim_create_buf(true, false)
  vim.bo[buf].buftype = "acwrite"
  vim.bo[buf].swapfile = false
  vim.api.nvim_buf_set_name(buf, "wiki://" .. content.id)
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, body_to_lines(content.body))
  set_metadata(buf, content)
  vim.bo[buf].modified = false
  vim.api.nvim_set_current_buf(buf)
  lsp_start()

  vim.api.nvim_create_autocmd("BufWriteCmd", {
    buffer = buf,
    callback = function()
      local body = lines_to_body(buf)
      local args = { "save", content.id, "--rev", tostring(vim.b[buf].wiki_rev) }
      if vim.b[buf].wiki_type == "page" then
        table.insert(args, "--auto-convert")
      end
      local ok, response = pcall(run, args, body)
      if not ok then
        vim.notify("wiki: save failed (conflict?): " .. tostring(response), vim.log.levels.ERROR)
        return
      end
      local saved = response.page or response
      vim.api.nvim_buf_set_var(buf, "wiki_rev", saved.rev)
      if saved.body and saved.body ~= body then
        vim.api.nvim_buf_set_lines(buf, 0, -1, false, body_to_lines(saved.body))
      end
      vim.bo[buf].modified = false
      local message = ("wiki: saved %s (rev %d)"):format(saved.title, saved.rev)
      if response.converted and #response.converted > 0 then
        message = message .. (", %d fences converted"):format(#response.converted)
      end
      if response.failed and #response.failed > 0 then
        message = message .. (", %d fences failed"):format(#response.failed)
        vim.notify("wiki: fence conversion: " .. table.concat(response.failed, "; "), vim.log.levels.WARN)
      end
      vim.notify(message, vim.log.levels.INFO)
    end,
  })

  vim.api.nvim_create_autocmd({ "TextChanged", "TextChangedI", "InsertLeave" }, {
    buffer = buf,
    callback = function()
      schedule_autosave(buf)
    end,
  })
  vim.api.nvim_create_autocmd("BufWipeout", {
    buffer = buf,
    callback = function()
      cancel_autosave(buf)
    end,
  })
  vim.notify(("wiki: opened %s"):format(content.title), vim.log.levels.INFO)
end

local function new_page(title)
  title = (title and title ~= "") and title or "無題のページ"
  local content = run({ "create_page", title }, "# " .. title .. "\n\n")
  open(content.id)
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

local function search(query)
  local data = run({ "search", query })
  local items = {}
  for _, item in ipairs(data.items or {}) do
    table.insert(items, {
      filename = "wiki://" .. item.id,
      lnum = 1,
      text = ("[%s] %s — %s"):format(item.type, item.title, item.excerpt or ""),
    })
  end
  vim.fn.setqflist(items)
  vim.cmd("copen")
end

local function workspace(id_or_title)
  local id = resolve_id(id_or_title)
  local ws = run({ "workspace", id })
  local items = {}
  for _, section in ipairs(ws.outline or {}) do
    table.insert(items, {
      filename = "wiki://" .. ws.page.id,
      lnum = section.line,
      text = ("%s%s"):format(string.rep("  ", section.level - 1), section.title),
    })
  end
  for _, link in ipairs((ws.links or {}).two_hop or {}) do
    table.insert(items, {
      filename = "wiki://" .. link.target.id,
      lnum = 1,
      text = ("2-hop via %s: %s"):format(link.via.title, link.target.title),
    })
  end
  vim.fn.setqflist(items)
  vim.cmd("copen")
end

vim.api.nvim_create_user_command("WikiOpen", function(args)
  if args.args == "" then
    local id = vim.b.wiki_id
    if not id then
      error("WikiOpen: content id or title required")
    end
    open(id)
    return
  end
  open(args.args)
end, { nargs = "?" })

vim.api.nvim_create_user_command("WikiNew", function(args)
  new_page(args.args)
end, { nargs = "?" })
vim.api.nvim_create_user_command("WikiList", list, {})
vim.api.nvim_create_user_command("WikiSearch", function(args)
  search(args.args)
end, { nargs = 1 })
vim.api.nvim_create_user_command("WikiWorkspace", function(args)
  workspace(args.args)
end, { nargs = 1 })
vim.api.nvim_create_user_command("WikiLspStart", lsp_start, {})
vim.api.nvim_create_user_command("WikiSave", function()
  vim.cmd("write")
end, {})
