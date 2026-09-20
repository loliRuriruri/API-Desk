import Database from "@tauri-apps/plugin-sql";

export const DB_URL = "sqlite:apidb.db";

let connection: Promise<Database> | null = null;

export function getDb(): Promise<Database> {
  if (!connection) {
    connection = Database.load(DB_URL);
  }
  return connection;
}

export async function select<T>(sql: string, params: unknown[] = []): Promise<T[]> {
  const db = await getDb();
  return db.select<T[]>(sql, params);
}

export async function execute(
  sql: string,
  params: unknown[] = [],
): Promise<{ rowsAffected: number; lastInsertId?: number }> {
  const db = await getDb();
  return db.execute(sql, params);
}

export async function selectOne<T>(sql: string, params: unknown[] = []): Promise<T | null> {
  const rows = await select<T>(sql, params);
  return rows.length > 0 ? rows[0] : null;
}
