const assertBatch = rows => { if (!Array.isArray(rows) || rows.length !== 2 || rows.some(row => row?.success !== true || !Array.isArray(row.results)) || !Number.isSafeInteger(rows[0]?.meta?.changes) || rows[0].meta.changes < 0 || rows[0].meta.changes > 1) throw new Error("audit_receipt_unknown"); };
const COLUMNS = ["event_name","source_instance","outcome","severity","actor_kind","actor_id","scope_module","scope_type","scope_id","resource_type","resource_id","correlation_id"];
export const SCHEMA_VERSION = 1;
const DDL = [
  "CREATE TABLE IF NOT EXISTS lenso_audit_schema(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL)",
  `CREATE TABLE IF NOT EXISTS audit_events(id TEXT PRIMARY KEY,occurred_key TEXT NOT NULL,event_json TEXT NOT NULL,${COLUMNS.map(k=>`${k} TEXT`).join(",")})`,
  "CREATE INDEX IF NOT EXISTS audit_event_order ON audit_events(occurred_key DESC,id DESC)",
  "INSERT INTO lenso_audit_schema(singleton,version) VALUES(1,1) ON CONFLICT(singleton) DO NOTHING",
];
// Explicit Owner operator action; create() and request handlers never run DDL.
export async function setup(database) {
  const session=database.withSession("first-primary");
  const exists=await session.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name=?").bind("lenso_audit_schema").first();
  if(!exists && await session.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name=?").bind("audit_events").first()) throw new Error("audit_schema_unmanaged");
  if(exists && (await session.prepare("SELECT version FROM lenso_audit_schema WHERE singleton=1").first())?.version!==SCHEMA_VERSION) throw new Error("audit_schema_incompatible");
  await session.batch(DDL.map(sql=>session.prepare(sql)));
  const row=await session.prepare("SELECT version FROM lenso_audit_schema WHERE singleton=1").first();
  if(row?.version!==SCHEMA_VERSION) throw new Error("audit_schema_incompatible");
}
export function create(database,scope,configuration) {
  if(typeof database?.withSession!=="function" || typeof scope?.run!=="function" || configuration?.profile!=="workers-d1") throw new Error("invalid_audit_d1_facility");
  const run=work=>scope.run(()=>work(database.withSession("first-primary")));
  const decode=row=>row ? JSON.parse(row.event_json) : null;
  return Object.freeze({
    fresh_id:()=>`audit_evt_${crypto.randomUUID()}`,
    readiness:()=>run(async(session)=>{ const row=await session.prepare("SELECT version FROM lenso_audit_schema WHERE singleton=1").first();if(row?.version!==1) throw new Error("audit_setup_required");return true; }),
    append:input=>run(async(session)=>{
      const event={...input.event,created_at:new Date().toISOString()};
      const values=[event.id,input.occurred_key,JSON.stringify(event),...COLUMNS.map(k=>event[k]??null)];
      const results=await session.batch([
        session.prepare(`INSERT INTO audit_events(id,occurred_key,event_json,${COLUMNS.join(",")}) VALUES(${values.map(()=>"?").join(",")}) ON CONFLICT(id) DO NOTHING`).bind(...values),
        session.prepare("SELECT event_json FROM audit_events WHERE id=?").bind(event.id),
      ]);
      assertBatch(results);
      const stored=decode(results[1]?.results?.[0]);if(!stored)throw new Error("audit_append_unknown");return stored;
    }),
    get:id=>run(async(session)=>decode(await session.prepare("SELECT event_json FROM audit_events WHERE id=?").bind(id).first())),
    list:filter=>run(async(session)=>{
      const conditions=[],values=[];
      for(const column of COLUMNS) if(filter[column]!=null){conditions.push(`${column}=?`);values.push(filter[column]);}
      if(filter.occurred_after!=null){conditions.push("occurred_key>=?");values.push(filter.occurred_after);}
      if(filter.occurred_before!=null){conditions.push("occurred_key<=?");values.push(filter.occurred_before);}
      if(filter.cursor){conditions.push("(occurred_key<? OR (occurred_key=? AND id<?))");values.push(filter.cursor.occurred_at,filter.cursor.occurred_at,filter.cursor.id);}
      if(!Number.isSafeInteger(filter.limit)||filter.limit<1||filter.limit>200)throw new Error("invalid_audit_limit");
      const query=`SELECT event_json FROM audit_events${conditions.length?" WHERE "+conditions.join(" AND "):""} ORDER BY occurred_key DESC,id DESC LIMIT ?`;
      const rows=await session.prepare(query).bind(...values,filter.limit+1).all();
      if(rows?.success!==true||!Array.isArray(rows.results))throw new Error("audit_read_unavailable");
      return rows.results.map(decode);
    }),
  });
}
