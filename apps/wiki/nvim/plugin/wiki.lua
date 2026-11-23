-- Wiki Neovim integration.
-- The server is the only component that holds Harmion credentials. Neovim
-- talks to it over a Unix socket and uses wiki://<id> virtual buffers.

local function server_cmd()
  local cmd = vim.g.wiki_server_cmd
    or (vim.env.WIKI_SERVER_CMD and { vim.env.WIKI_SERVER_CMD })
    or { "wiki-server" }
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
  vim.api.nvim_buf_set_var(buf, "wiki_title", content.title)
  vim.o.title = true
  vim.o.titlestring = "Harmion Wiki — " .. content.title
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
      timer:close()
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

local function record_conflict(buf, id)
  local ok, current = pcall(run, { "open", id })
  if not ok then
    return
  end
  vim.api.nvim_buf_set_var(buf, "wiki_conflict_body", current.body)
  vim.schedule(function()
    if vim.api.nvim_buf_is_valid(buf) then
      vim.bo[buf].modified = true
    end
  end)
  vim.notify("wiki: conflict — :WikiConflict で現在の本文と比較できます", vim.log.levels.ERROR)
end

local function open(id_or_title)
  local id = resolve_id(id_or_title)
  local existing = vim.fn.bufnr("wiki://" .. id)
  if existing > 0 then
    vim.api.nvim_set_current_buf(existing)
    if not vim.bo[existing].modified then
      local fresh = run({ "open", id })
      vim.api.nvim_buf_set_lines(existing, 0, -1, false, body_to_lines(fresh.body))
      set_metadata(existing, fresh)
      vim.bo[existing].modified = false
    end
    lsp_start()
    vim.notify(("wiki: switched to %s"):format(vim.b[existing].wiki_title), vim.log.levels.INFO)
    return
  end
  local content = run({ "open", id })
  local buf = vim.api.nvim_create_buf(true, false)
  vim.bo[buf].buftype = "acwrite"
  -- Keep a swap file so a crash during a 1.5s autosave window is recoverable.
  vim.bo[buf].swapfile = true
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
        record_conflict(buf, content.id)
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
local function current_id()
  local id = vim.b.wiki_id
  if not id then
    error("wiki: current buffer is not a Wiki Content")
  end
  return id
end

vim.api.nvim_create_user_command("WikiTitle", function(args)
  local updated = run({ "update_meta", current_id(), "--rev", tostring(vim.b.wiki_rev), "--title", args.args })
  vim.api.nvim_buf_set_var(0, "wiki_title", updated.title)
  vim.api.nvim_buf_set_var(0, "wiki_rev", updated.rev)
  vim.notify("wiki: title = " .. updated.title, vim.log.levels.INFO)
end, { nargs = 1 })

vim.api.nvim_create_user_command("WikiTags", function(args)
  local updated = run({ "update_meta", current_id(), "--rev", tostring(vim.b.wiki_rev), "--tags", args.args })
  vim.api.nvim_buf_set_var(0, "wiki_rev", updated.rev)
  vim.b.wiki_tags = table.concat(updated.tags or {}, ",")
  vim.notify("wiki: tags = " .. (vim.b.wiki_tags ~= "" and vim.b.wiki_tags or "(none)"), vim.log.levels.INFO)
end, { nargs = 1 })

vim.api.nvim_create_user_command("WikiDelete", function(args)
  local id = current_id()
  if args.bang or vim.fn.confirm("Delete " .. id .. "?", "&Yes\n&No", 2) == 1 then
    run({ "delete", id, "--rev", tostring(vim.b.wiki_rev) })
    vim.cmd("bwipeout!")
  end
end, { bang = true })

vim.api.nvim_create_user_command("WikiList", list, {})
vim.api.nvim_create_user_command("WikiSearch", function(args)
  search(args.args)
end, { nargs = 1 })
vim.api.nvim_create_user_command("WikiWorkspace", function(args)
  workspace(args.args)
end, { nargs = 1 })
vim.api.nvim_create_user_command("WikiConflict", function()
  local id = current_id()
  local current = run({ "open", id })
  local local_body = lines_to_body(0)
  local remote_path = vim.fn.tempname()
  local local_path = vim.fn.tempname()
  vim.fn.writefile(vim.split(current.body, "\n", { plain = true }), remote_path)
  vim.fn.writefile(vim.split(local_body, "\n", { plain = true }), local_path)
  vim.cmd("tabnew " .. vim.fn.fnameescape(remote_path))
  vim.cmd("vert diffsplit " .. vim.fn.fnameescape(local_path))
  vim.cmd("windo diffthis")
  vim.cmd("windo setlocal filetype=markdown")
end, {})

vim.api.nvim_create_user_command("WikiLspStart", lsp_start, {})
vim.api.nvim_create_user_command("WikiSave", function()
  vim.cmd("write")
end, {})

-- Quickfix entries and LSP locations use `wiki://<id>`. Without this handler
-- Neovim would try to open a real file with that name.
vim.api.nvim_create_autocmd("BufReadCmd", {
  pattern = "wiki://*",
  callback = function(args)
    local uri = vim.api.nvim_buf_get_name(args.buf)
    local id = uri:match("^wiki://(.+)$")
    if not id then
      return
    end
    local ok, content = pcall(run, { "open", id })
    if not ok then
      vim.notify("wiki: cannot open " .. uri .. ": " .. tostring(content), vim.log.levels.ERROR)
      vim.bo[args.buf].buftype = "nofile"
      return
    end
    vim.bo[args.buf].buftype = "acwrite"
    vim.bo[args.buf].swapfile = true
    vim.api.nvim_buf_set_lines(args.buf, 0, -1, false, body_to_lines(content.body))
    set_metadata(args.buf, content)
    vim.bo[args.buf].modified = false
    vim.api.nvim_create_autocmd("BufWriteCmd", {
      buffer = args.buf,
      callback = function()
        local body = lines_to_body(args.buf)
        local argv = { "save", content.id, "--rev", tostring(vim.b[args.buf].wiki_rev) }
        if vim.b[args.buf].wiki_type == "page" then
          table.insert(argv, "--auto-convert")
        end
        local saved_ok, response = pcall(run, argv, body)
        if not saved_ok then
          vim.notify("wiki: save failed: " .. tostring(response), vim.log.levels.ERROR)
          record_conflict(args.buf, content.id)
          return
        end
        local saved = response.page or response
        vim.api.nvim_buf_set_var(args.buf, "wiki_rev", saved.rev)
        if saved.body and saved.body ~= body then
          vim.api.nvim_buf_set_lines(args.buf, 0, -1, false, body_to_lines(saved.body))
        end
        vim.bo[args.buf].modified = false
      end,
    })
    lsp_start()
  end,
})
