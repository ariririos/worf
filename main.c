#define __STDC_WANT_LIB_EXT1 1
#include <assert.h>
#include <errno.h>
#include <mpd/client.h>
#include <sqlite3.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#define NUM_FEATURES 20
#define DEBUG

const char *MPD_MUSIC_DIR = "/media/aririos/Files/zotify/"; // TODO: use mpd_send_list_mounts to retrieve

typedef struct {
  enum mpd_entity_type type;
  time_t mtime;
  time_t atime;
  char *path;
  int song_id;
} DatabaseEntity;
enum BLISS_FEATURES {
  TEMPO,
  ZERO_CROSSING_RATE,
  MEAN_SPECTRAL_CENTROID,
  STD_DEV_SPECTRAL_CENTROID,
  MEAN_SPECTRAL_ROLLOFF,
  STD_DEV_SPECTRAL_ROLLOFF,
  MEAN_SPECTRAL_FLATNESS,
  STD_DEV_SPECTRAL_FLATNESS,
  MEAN_LOUDNESS,
  STD_DEV_LOUDNESS,
  CHROMA_INTERVAL_ONE,
  CHROMA_INTERVAL_TWO,
  CHROMA_INTERVAL_THREE,
  CHROMA_INTERVAL_FOUR,
  CHROMA_INTERVAL_FIVE,
  CHROMA_INTERVAL_SIX,
  CHROMA_INTERVAL_SEVEN,
  CHROMA_INTERVAL_EIGHT,
  CHROMA_INTERVAL_NINE,
  CHROMA_INTERVAL_TEN
};

static int clear_callback(void *_, int argc, char **argv, char **col_name);
static int dup_callback(void *dup_found_bool_ptr, int argc, char **argv, char **col_name);
static int song_id_callback(void *song_id_ptr, int argc, char **argv, char **col_name);
char *replace_all(char *str, const char *substr, const char *replacement);
void get_all_mpd_songs(struct mpd_connection *conn, sqlite3 *mpd_db, sqlite3 *bliss_db);
void get_all_songs_since_timestamp(struct mpd_connection *conn, struct timespec *timestamp, sqlite3 *mpd_db, sqlite3 *bliss_db);
bool check_for_dup(struct mpd_connection *conn, DatabaseEntity *db_entity, sqlite3 *mpd_db);
bool add_to_mpd_db(DatabaseEntity *db_entity, sqlite3 *mpd_db, sqlite3 *bliss_db);
char *replace_all(char *str, const char *substr, const char *replacement);
int get_song_id(DatabaseEntity *db_entity, sqlite3 *bliss_db);

static int clear_callback(void *_, int argc, char **argv, char **col_name) {
  printf("mpd.db cleared\n");
  return 0;
}

static int dup_callback(void *dup_found_ptr, int argc, char **argv, char **col_name) {
  bool *dup_found_bool_ptr = dup_found_ptr;
  if (argc > 0) {
    *dup_found_bool_ptr = true;
  } else {
    *dup_found_bool_ptr = false;
  }
  return 0;
}

static int song_id_callback(void *song_id_ptr, int argc, char **argv, char **col_name) {
  char *end_ptr;
  errno = 0; 
  int *song_id_int_ptr = song_id_ptr;
  *song_id_int_ptr = strtol(argv[0], &end_ptr, 10);
  if (errno == ERANGE || *end_ptr != '\0' || end_ptr == argv[0]) {
    fprintf(stderr, "strtol failed at song_id_callback\n");
    return 1;
  }
  return 0;
}

void get_all_mpd_songs(struct mpd_connection *conn, sqlite3 *mpd_db, sqlite3 *bliss_db) {
  // Clear database
  char *sql_clear_mpd_db = "DROP TABLE entities; CREATE TABLE entities(type INT, path text primary key, atime INT, mtime INT, song_id INT unique);";
  char *z_err = 0;
  int mpd_rc = sqlite3_exec(mpd_db, sql_clear_mpd_db, clear_callback, 0, &z_err);
  if (mpd_rc != SQLITE_OK) {
    fprintf(stderr, "sqlite exec error at mpd_db clear: %s\n", z_err);
    free(z_err); 
    return;
  }
  printf("Getting stats...\n");
  struct mpd_stats *stats = mpd_run_stats(conn);
  if (stats == NULL) {
    fprintf(stderr, "Out of memory at mpd_run_stats\n");
    return;
  }
  int mpd_total = mpd_stats_get_number_of_artists(stats) +
                  mpd_stats_get_number_of_albums(stats) +
                  mpd_stats_get_number_of_songs(stats);
  mpd_stats_free(stats);
  printf("Getting all %d items, this will take a while...\n", mpd_total);
  if (mpd_send_list_all(conn, "")) {
    struct mpd_entity *entity;
    while ((entity = mpd_recv_entity(conn))) {
      DatabaseEntity db_entity = {.atime = 0, .mtime = 0, .type = mpd_entity_get_type(entity), .path = "", .song_id = 0};
      switch (db_entity.type) {
        case MPD_ENTITY_TYPE_DIRECTORY:
          mpd_entity_free(entity);
          continue;
          // const struct mpd_directory *dir = mpd_entity_get_directory(entity);
          // db_entity.path = strdup(mpd_directory_get_path(dir));
          // db_entity.mtime = mpd_directory_get_last_modified(dir);
        break;
        case MPD_ENTITY_TYPE_PLAYLIST:
          mpd_entity_free(entity);
          continue;
          // const struct mpd_playlist *playlist = mpd_entity_get_playlist(entity);
          // db_entity.path = strdup(mpd_playlist_get_path(playlist));
          // db_entity.mtime = mpd_playlist_get_last_modified(playlist);
        break;
        case MPD_ENTITY_TYPE_SONG:
            const struct mpd_song *song = mpd_entity_get_song(entity);
            db_entity.path = strdup(mpd_song_get_uri(song)); // FIXME: small memory leak
            db_entity.mtime = mpd_song_get_last_modified(song);
            db_entity.atime = mpd_song_get_added(song);
        break;
        case MPD_ENTITY_TYPE_UNKNOWN:
        default:
            fprintf(stderr, "Unknown entity type: %d\n", db_entity.type);
            mpd_entity_free(entity);
            return;
        break;
      }
      db_entity.path = replace_all(db_entity.path, "'", "''");
      if (db_entity.path == NULL) {
        fprintf(stderr, "db_entity.path replace_all failed\n");
        mpd_entity_free(entity);
        return;
      }
      if (db_entity.type == MPD_ENTITY_TYPE_SONG) {
        db_entity.song_id = get_song_id(&db_entity, bliss_db);
        if (db_entity.song_id == 0) {
          fprintf(stderr, "Failed to get bliss song_id for %s\n", db_entity.path);
        }
      }
      if (check_for_dup(conn, &db_entity, mpd_db)) {
        fprintf(stderr, "Duplicate found for %s\n", db_entity.path);
        free(db_entity.path);
        mpd_entity_free(entity);
        continue;
      }
      if (!add_to_mpd_db(&db_entity, mpd_db, bliss_db)) {
        fprintf(stderr, "add_to_mpd_db failed\n");
        free(db_entity.path);
        mpd_entity_free(entity);
        return;
      }
      free(db_entity.path);
      mpd_entity_free(entity);
    }
    printf("Success!\n");
    if (mpd_connection_get_error(conn) != MPD_ERROR_SUCCESS) {
      fprintf(stderr, "mpd connection error at recv_entity: %s\n", mpd_connection_get_error_message(conn));
      return;
    }
  }
  else {
    fprintf(stderr, "mpd connection error at send_list_all: %s\n", mpd_connection_get_error_message(conn));
    return;
  }
}

void get_all_songs_since_timestamp(struct mpd_connection *conn, struct timespec *timestamp, sqlite3 *mpd_db, sqlite3 *bliss_db) {
  char time_str[256];
  strftime(time_str, sizeof(time_str), "%FT%T%z", localtime(&timestamp->tv_sec));
  printf("Getting items added since %s, this will take a while...\n", time_str);
  if (mpd_search_db_songs(conn, true) &&
      mpd_search_add_added_since_constraint(conn, MPD_OPERATOR_DEFAULT, (time_t)timestamp->tv_sec) &&
      mpd_search_commit(conn)) {
    struct mpd_song *song;
    while ((song = mpd_recv_song(conn))) {
      DatabaseEntity db_entity = {
        .atime = mpd_song_get_added(song),
        .mtime = mpd_song_get_last_modified(song),
        .type = MPD_ENTITY_TYPE_SONG,
        .path = strdup(mpd_song_get_uri(song)),
        .song_id = 0
      };
      db_entity.path = replace_all(db_entity.path, "'", "''");
      if (db_entity.path == NULL) {
        fprintf(stderr, "db_entity.path replace_all failed\n");
        mpd_song_free(song);
        return;
      }
      db_entity.song_id = get_song_id(&db_entity, bliss_db);
      if (db_entity.song_id == 0) {
        fprintf(stderr, "Failed to get bliss song_id for %s\n", db_entity.path);
      }
      if (check_for_dup(conn, &db_entity, mpd_db)) {
        fprintf(stderr, "Duplicate found for %s\n", db_entity.path);
        free(db_entity.path);
        mpd_song_free(song);
        continue;
      }
      if (!add_to_mpd_db(&db_entity, mpd_db, bliss_db)) {
        fprintf(stderr, "add_to_mpd_db failed\n");
        free(db_entity.path);
        mpd_song_free(song);
        return;
      }
      free(db_entity.path);
      mpd_song_free(song);
    }
    printf("Success!\n");
    if (mpd_connection_get_error(conn) != MPD_ERROR_SUCCESS) {
      fprintf(stderr, "mpd connection error at recv_entity: %s\n", mpd_connection_get_error_message(conn));
      mpd_song_free(song);
      return;
    }
  } else {
    fprintf(stderr, "No new songs\n");
    return;
  }
}

bool check_for_dup(struct mpd_connection *conn, DatabaseEntity *db_entity, sqlite3 *mpd_db) {
  char *z_err = 0;
  char *check_for_dup_prefix = "select * from entities where path = ";
  int dup_check_len = strlen(check_for_dup_prefix) + strlen(db_entity->path) + 3;
  char *check_for_dup = malloc(dup_check_len);
  snprintf(check_for_dup, dup_check_len, "%s'%s'", check_for_dup_prefix, db_entity->path);
  bool dup_found;
  int mpd_rc = sqlite3_exec(mpd_db, check_for_dup, dup_callback, &dup_found, &z_err);
  if (mpd_rc != SQLITE_OK) {
    fprintf(stderr, "sqlite check_for_dup exec error: %s\n", z_err);
    free(z_err);
    free(check_for_dup);
    free(db_entity->path);
    mpd_connection_free(conn);
    exit(1);
  }
  free(check_for_dup);
  return dup_found;
}

bool add_to_mpd_db(DatabaseEntity *db_entity, sqlite3 *mpd_db, sqlite3 *bliss_db) {
  char *z_err = 0;
  char *sql_header = "insert into entities(type, path, mtime, atime, song_id) values ";
  int len = strlen(sql_header) + snprintf(NULL, 0, "%d", db_entity->type) + 1 + strlen(db_entity->path) + 1 + snprintf(NULL, 0, "%ld", db_entity->mtime) + 1 + snprintf(NULL, 0, "%ld", db_entity->atime) + 1 + snprintf(NULL, 0, "%d", db_entity->song_id) + 6; // 6 = two parens, a semicolon, two quotes, and a null
  char *row = malloc(len);
  if (row == NULL) {
    fprintf(stderr, "Out of memory at malloc row\n");
    return false;
  }
  snprintf(row, len, "%s(%d,'%s',%ld,%ld,%d);", sql_header, db_entity->type, db_entity->path, db_entity->mtime, db_entity->atime, db_entity->song_id);
  #ifdef DEBUG
  printf("%s\n", row);
  #endif
  int mpd_rc = sqlite3_exec(mpd_db, row, clear_callback, 0, &z_err);
  if (mpd_rc != SQLITE_OK) {
    fprintf(stderr, "sqlite exec error: %s\n", z_err);
    free(z_err);
    free(row);
    return false;
  }
  free(row);
  return true;
}

char *replace_all(char *str, const char *substr, const char *replacement) {
  if (strcmp(str, "") == 0 || strcmp(substr, "") == 0) {
    fprintf(stderr, "Empty string or substring not supported");
    return NULL;
  }
  char *pos = strstr(str, substr);
  if (pos == NULL) {
    return str;
  }
  char *last;
  char *out = calloc(strlen(str) + 1, sizeof(char));
  if (out == NULL) {
    fprintf(stderr, "Out of memory at replace_all malloc");
    return NULL;
  }
  char *new_out;
  ptrdiff_t init_diff = pos - str + 1;
  snprintf(out, init_diff, "%s", str);

  while (pos) {
    int new_len = strlen(out) + strlen(replacement) + 1;
    new_out = realloc(out, new_len);
    if (new_out == NULL) {
      free(out);
      fprintf(stderr, "Out of memory at replace_all realloc");
      return NULL;
    }
    out = new_out;
    strcat(out, replacement);
    last = pos + strlen(substr);
    pos = strstr(last, substr);
    if (pos) {
      ptrdiff_t next_diff = pos - last;
      new_out = realloc(out, new_len + next_diff);
      if (new_out == NULL) {
        free(out);
        fprintf(stderr, "Out of memory at replace_all if pos realloc");
        return NULL;
      }
      out = new_out;
      strncat(out, last, next_diff);
    } else {
      new_out = realloc(out, new_len + strlen(last));
      if (new_out == NULL) {
        free(out);
        fprintf(stderr, "Out of memory at replace_all else realloc");
        return NULL;
      }
      out = new_out;
      strcat(out, last);
    }
  }

  return out;
}

int get_song_id(DatabaseEntity *db_entity, sqlite3 *bliss_db) {
  char *z_err = 0;
  char *sql_header = "select * from song where path = ";
  int len = strlen(sql_header) + strlen(MPD_MUSIC_DIR) + strlen(db_entity->path) + 4; // 4 = two quotes, semicolon, and a null
  char *sql_query = malloc(len);
  if (sql_query == NULL) {
    fprintf(stderr, "Out of memory at get_song_id malloc");
  }
  snprintf(sql_query, len, "%s'%s%s';", sql_header, MPD_MUSIC_DIR, db_entity->path);
  int song_id = 0;
  int bliss_rc = sqlite3_exec(bliss_db, sql_query, song_id_callback, &song_id, &z_err);
  if (bliss_rc != SQLITE_OK) {
    fprintf(stderr, "sqlite exec error: %s\n", z_err);
    free(z_err);
    free(sql_query);
    return 0;
  }
  free(sql_query);
  return song_id;
}

int main(int argc, char **argv) {
  sqlite3 *mpd_db;
  sqlite3 *bliss_db;
  FILE *timestamp_fp;
  bool empty_timestamp = true;
  struct mpd_connection *conn = mpd_connection_new(NULL, 0, 300000);
  if (conn == NULL) {
    fprintf(stderr, "Out of memory at mpd_connection_new\n");
    return 1;
  }
  mpd_connection_set_keepalive(conn, true);
  if (argv[1]) {
    if (!mpd_run_password(conn, argv[1])) {
      fprintf(stderr, "Bad password\n");
      mpd_connection_free(conn);
      return 1;
    }
  }
  int mpd_rc = sqlite3_open("mpd.db", &mpd_db);
  if (mpd_rc) {
    fprintf(stderr, "sqlite error at open mpd_db: %s\n", sqlite3_errmsg(mpd_db));
    return 1;
  }
  int bliss_rc = sqlite3_open("/home/aririos/.config/bliss-rs/songs.db", &bliss_db);
  if (bliss_rc) {
    fprintf(stderr, "sqlite error at open bliss_db: %s\n", sqlite3_errmsg(bliss_db));
    mpd_connection_free(conn);
    int db_close_status = sqlite3_close(mpd_db);
    if (db_close_status != SQLITE_OK) {
      fprintf(stderr, "Failed to close mpd_db with error code %d\n", db_close_status);
    }
    return 1;
  }

  if (mpd_connection_get_error(conn) != MPD_ERROR_SUCCESS) {
    fprintf(stderr, "mpd connection error at init: %s\n", mpd_connection_get_error_message(conn));
    mpd_connection_free(conn);
    int mpd_db_close_status = sqlite3_close(mpd_db);
    if (mpd_db_close_status != SQLITE_OK) {
      fprintf(stderr, "Failed to close mpd_db with error code %d\n", mpd_db_close_status);
    }
    int bliss_db_close_status = sqlite3_close(bliss_db);
    if (bliss_db_close_status != SQLITE_OK) {
      fprintf(stderr, "Failed to close bliss_db with error code %d\n", bliss_db_close_status);
    }
    return 1;
  }

  timestamp_fp = fopen("timestamp", "r+");
  if (timestamp_fp == NULL) {
    timestamp_fp = fopen("timestamp", "w");
    if (timestamp_fp == NULL) {
      fprintf(stderr, "Failed to create timestamp file\n");
      return 1;
    }
    fclose(timestamp_fp);
    timestamp_fp = fopen("timestamp", "r+");
  }
  struct timespec last_ts = {.tv_nsec = 0, .tv_sec = 0};
  fscanf(timestamp_fp, "%jd", &last_ts.tv_sec);
  if (last_ts.tv_sec != 0) {
    empty_timestamp = false;
  }
  struct timespec cur_ts;
  timespec_get(&cur_ts, TIME_UTC);
  fseek(timestamp_fp, 0, SEEK_SET);
  fprintf(timestamp_fp, "%jd", (intmax_t)cur_ts.tv_sec);
  fclose(timestamp_fp);

  #ifdef DEBUG
  empty_timestamp = true;
  #endif

  char blissify_cmd[256];
  if (argv[1]) {
    snprintf(blissify_cmd, 255, "MPD_HOST='%s@127.0.0.1' blissify update", argv[1]);
  }
  else {
    snprintf(blissify_cmd, 255, "blissify update");
  }
  printf("Running `blissify update`...\n");
  int blissify_rc = system(blissify_cmd);
  if (blissify_rc != 0) {
    fprintf(stderr, "`blissify update` failed with exit code %d", blissify_rc);
    mpd_connection_free(conn);
    return 1;
  }

  if (empty_timestamp) {
    get_all_mpd_songs(conn, mpd_db, bliss_db);
  }
  else {
    get_all_songs_since_timestamp(conn, &last_ts, mpd_db, bliss_db);
  }

  printf("Closing...\n");
  mpd_connection_free(conn);
  int mpd_db_close_status = sqlite3_close(mpd_db);
  if (mpd_db_close_status != SQLITE_OK) {
    fprintf(stderr, "Failed to close mpd_db with error code %d\n", mpd_db_close_status);
  }
  int bliss_db_close_status = sqlite3_close(bliss_db);
  if (bliss_db_close_status != SQLITE_OK) {
    fprintf(stderr, "Failed to close bliss_db with error code %d\n", bliss_db_close_status);
  }

  return 0;
}
