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
// #define DEBUG 

const char *MPD_MUSIC_DIR =
    "/media/aririos/Files/zotify/"; // TODO: use mpd_send_list_mounts to retrieve
int mpd_total = 0;
int mpd_songs_total = 0;

typedef struct {
  enum mpd_entity_type type;
  time_t mtime;
  time_t atime;
  char *path;
  int song_id;
} DatabaseEntity;

typedef struct {
  int argc;
  char **argv;
  char **col_name;
} MPDDatabaseResponse;

typedef struct {
  int *i;
  struct mpd_connection *conn;
  sqlite3 *bliss_db;
  sqlite3 *mpd_db;
} VerifySongController;

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

bool int_in_arr(int needle, int *haystack, size_t len);
int str_pos_in_arr(char *needle, char **haystack, size_t len);
char *replace_all(char *str, const char *substr, const char *replacement);
static int clear_callback(void *_, int argc, char **argv, char **col_name);
static int dup_callback(void *dup_found, int argc, char **argv, char **col_name);
static int song_id_callback(void *song_id, int argc, char **argv, char **col_name);
static int song_query_callback(void *db_entity, int argc, char **argv, char **col_name);
static int verify_song_callback(void *controller, int argc, char **argv, char **col_name);
void get_all_mpd_songs(struct mpd_connection *conn, sqlite3 *mpd_db, sqlite3 *bliss_db);
void get_all_songs_since_timestamp(struct mpd_connection *conn, struct timespec *timestamp, sqlite3 *mpd_db, sqlite3 *bliss_db);
bool check_for_dup(struct mpd_connection *conn, DatabaseEntity *db_entity, sqlite3 *mpd_db);
bool add_to_mpd_db(DatabaseEntity *db_entity, sqlite3 *mpd_db, sqlite3 *bliss_db);
int get_song_id(DatabaseEntity *db_entity, sqlite3 *bliss_db);
int random_playlist(struct mpd_connection *conn, sqlite3 *mpd_db, sqlite3 *bliss_db, size_t playlist_len);
void db_entities_free(DatabaseEntity **db_entities, size_t len);
int add_mpd_db_songs_from_bliss_db(VerifySongController *controller, int mpd_db_id, MPDDatabaseResponse *db_response);
int verify_mpd_db_song_against_bliss_db(VerifySongController *controller, int mpd_db_id, MPDDatabaseResponse *db_response);
int verify_mpd_db(struct mpd_connection *conn, sqlite3 *mpd_db, sqlite3 *bliss_db);

bool int_in_arr(int needle, int *haystack, size_t len) {
  for (size_t i = 0; i < len; i++) {
    if (haystack[i] == needle) {
      return true;
    }
  }
  return false;
}

int str_pos_in_arr(char *needle, char **haystack, size_t len) {
  for (size_t i = 0; i < len; i++) {
    if (strcmp(haystack[i], needle) == 0) {
      return i;
    }
  }
  return -1;
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
    }
    else {
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

static int clear_callback(void *_, int argc, char **argv, char **col_name) {
  return 0;
}

static int dup_callback(void *dup_found, int argc, char **argv, char **col_name) {
  bool *dup_found_ptr = dup_found;
  *dup_found_ptr = true;
  return 0;
}

static int song_id_callback(void *song_id, int argc, char **argv, char **col_name) {
  char *end_ptr;
  errno = 0;
  int *song_id_ptr = song_id;
  int id_col_pos = str_pos_in_arr("id", col_name, argc);
  if (id_col_pos == -1) {
    fprintf(stderr, "id column not found at song_id_callback\n");
    return 1;
  }
  *song_id_ptr = strtol(argv[id_col_pos], &end_ptr, 10);
  if (errno == ERANGE || *end_ptr != '\0' || end_ptr == argv[0]) {
    fprintf(stderr, "strtol failed at song_id_callback\n");
    return 1;
  }
  return 0;
}

static int song_query_callback(void *db_entity, int argc, char **argv, char **col_name) {
  DatabaseEntity *db_entity_ptr = db_entity;
  int path_col_pos = str_pos_in_arr("path", col_name, argc);
  if (path_col_pos == -1) {
    fprintf(stderr, "path column not found at song_query_callback\n");
    return 1;
  }
  int len = strlen(argv[path_col_pos]) + 1;
  db_entity_ptr->path = malloc(len);
  if (db_entity_ptr->path == NULL) {
    fprintf(stderr, "Out of memory at song_query_callback malloc\n");
    return 1;
  }
  strcpy(db_entity_ptr->path, argv[path_col_pos]);
  return 0;
}

static int verify_song_callback(void *controller, int argc, char **argv, char **col_name) {
  char *end_ptr;
  errno = 0;
  VerifySongController *controller_ptr = controller;
  int *i = controller_ptr->i;
  int song_id_col_pos = str_pos_in_arr("song_id", col_name, argc);
  if (song_id_col_pos == -1) {
    fprintf(stderr, "song_id column not found at verify_song_callback\n");
    return 1;
  }
  int mpd_db_id = strtol(argv[song_id_col_pos], &end_ptr, 10);
  if (errno == ERANGE || *end_ptr != '\0' || end_ptr == argv[song_id_col_pos]) {
    fprintf(stderr, "strtol failed at verify_song_callback\n");
    return 1;
  }
  MPDDatabaseResponse *db_response = malloc(sizeof(MPDDatabaseResponse));
  if (db_response == NULL) {
    fprintf(stderr, "Out of memory at verify_song_callback db_response malloc\n");
    return 1;
  }
  db_response->argc = argc;
  db_response->argv = argv;
  db_response->col_name = col_name;
  if (mpd_db_id != *i) { // song(s) not in mpd_db, add it/them from bliss_db
    return add_mpd_db_songs_from_bliss_db(controller, mpd_db_id, db_response);
  }
  else { // song in mpd_db, verify path matches bliss_db
    return verify_mpd_db_song_against_bliss_db(controller, mpd_db_id, db_response);
  }
}

void get_all_mpd_songs(struct mpd_connection *conn, sqlite3 *mpd_db, sqlite3 *bliss_db) {
  char *sql_clear_mpd_db = "DROP TABLE entities; CREATE TABLE entities(type INT, "
                           "path text primary key, atime INT, mtime INT, "
                           "song_id INT unique);";
  char *z_err = 0;
  int mpd_rc = sqlite3_exec(mpd_db, sql_clear_mpd_db, clear_callback, 0, &z_err);
  if (mpd_rc != SQLITE_OK) {
    fprintf(stderr, "sqlite exec error at mpd_db clear: %s\n", z_err);
    free(z_err);
    return;
  }
  printf("Getting all %d items, this will take a while...\n", mpd_total);
  if (mpd_send_list_all(conn, "")) {
    struct mpd_entity *entity;
    while ((entity = mpd_recv_entity(conn))) {
      DatabaseEntity db_entity = {
        .atime = 0,
        .mtime = 0,
        .type = mpd_entity_get_type(entity),
        .path = "",
        .song_id = 0
      };
      switch (db_entity.type) {
      case MPD_ENTITY_TYPE_DIRECTORY:
        mpd_entity_free(entity);
        continue;
      case MPD_ENTITY_TYPE_PLAYLIST:
        mpd_entity_free(entity);
        continue;
      case MPD_ENTITY_TYPE_SONG:
        const struct mpd_song *song = mpd_entity_get_song(entity);
        db_entity.path = strdup(mpd_song_get_uri(song));
        db_entity.mtime = mpd_song_get_last_modified(song);
        db_entity.atime = mpd_song_get_added(song);
        break;
      case MPD_ENTITY_TYPE_UNKNOWN:
      default:
        fprintf(stderr, "Unknown entity type: %d\n", db_entity.type);
        mpd_entity_free(entity);
        return;
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
  printf("Getting items added since %s, this might take a while...\n", time_str);
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
  }
  else {
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
  int len = strlen(sql_header) + snprintf(NULL, 0, "%d", db_entity->type) + 1 +
            strlen(db_entity->path) + 1 +
            snprintf(NULL, 0, "%ld", db_entity->mtime) + 1 +
            snprintf(NULL, 0, "%ld", db_entity->atime) + 1 +
            snprintf(NULL, 0, "%d", db_entity->song_id) +
            6; // 6 = two parens, a semicolon, two quotes, and a null
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
    fprintf(stderr, "sqlite exec error for %s (song_id: %d) at add_to_mpd_db: %s\n", db_entity->path, db_entity->song_id, z_err);
    free(z_err);
    free(row);
    return false;
  }
  free(row);
  return true;
}

int get_song_id(DatabaseEntity *db_entity, sqlite3 *bliss_db) {
  char *z_err = 0;
  char *sql_header = "select * from song where path = ";
  int len = strlen(sql_header) + strlen(MPD_MUSIC_DIR) +
            strlen(db_entity->path) +
            4; // 4 = two quotes, semicolon, and a null
  char *sql_query = malloc(len);
  if (sql_query == NULL) {
    fprintf(stderr, "Out of memory at get_song_id malloc");
  }
  snprintf(sql_query, len, "%s'%s%s';", sql_header, MPD_MUSIC_DIR, db_entity->path);
  int song_id = 0;
  int bliss_rc = sqlite3_exec(bliss_db, sql_query, song_id_callback, &song_id, &z_err);
  if (bliss_rc != SQLITE_OK) {
    fprintf(stderr, "sqlite exec error at get_song_id: %s\n", z_err);
    free(z_err);
    free(sql_query);
    return 0;
  }
  free(sql_query);
  return song_id;
}

int random_playlist(struct mpd_connection *conn, sqlite3 *mpd_db, sqlite3 *bliss_db, size_t playlist_len) {
  char *z_err = 0;
  srand((unsigned) time(NULL));
  DatabaseEntity *db_entities[playlist_len];
  printf("Creating random playlist of length %jd...\n", playlist_len);
  int used_song_ids[playlist_len];
  memset(used_song_ids, 0, playlist_len * sizeof(int));
  for (int i = 0; i < playlist_len; i++) {
    int rand_song_id = rand() % mpd_songs_total + 1;
    while (int_in_arr(rand_song_id, used_song_ids, playlist_len)) {
      rand_song_id = rand() % mpd_songs_total + 1;
    }
    used_song_ids[i] = rand_song_id;
    DatabaseEntity *db_entity = malloc(sizeof(DatabaseEntity));
    db_entity->type = MPD_ENTITY_TYPE_SONG;
    db_entity->atime = 0;
    db_entity->mtime = 0;
    db_entity->song_id = rand_song_id;
    char *song_query_header = "select * from entities where song_id = ";
    int len = strlen(song_query_header) + snprintf(NULL, 0, "%d", db_entity->song_id) + 2;
    char *song_query = malloc(len);
    if (song_query == NULL) {
      fprintf(stderr, "Out of memory at random_playlist malloc\n");
      return 1;
    }
    snprintf(song_query, len, "%s%d;", song_query_header, db_entity->song_id);
    int mpd_rc = sqlite3_exec(mpd_db, song_query, song_query_callback, db_entity, &z_err);
    if (mpd_rc != SQLITE_OK) {
      fprintf(stderr, "sqlite exec error at random_playlist: %s\n", z_err);
      free(z_err);
      free(song_query);
      return 1;
    }
    if (db_entity->path == NULL) {
      fprintf(stderr, "Failed to add song with song_id %d at random_playlist\n", rand_song_id);
      free(song_query);
      free(db_entity);
      return 1;
    }
    db_entities[i] = db_entity;
    free(song_query);
  }

  const char *random_playlist_name = "random";
  bool rm_rc = mpd_run_rm(conn, random_playlist_name);
  if (!rm_rc) {
    enum mpd_error err = mpd_connection_get_error(conn);
    if (err == MPD_ERROR_SERVER) {
      enum mpd_server_error server_err = mpd_connection_get_server_error(conn);
      if (server_err != MPD_SERVER_ERROR_NO_EXIST) {
        fprintf(stderr, "mpd_run_rm server error code: %d", server_err);
        db_entities_free(db_entities, playlist_len);
        return 1;
      }
    }
    else {
      printf("mpd_run_rm error: %d\n", mpd_connection_get_error(conn));
      db_entities_free(db_entities, playlist_len);
      return 1;
    }
  }
  for (int i = 0; i < playlist_len; i++) {
    bool playlist_add_rc = mpd_send_playlist_add(conn, random_playlist_name, db_entities[i]->path);
    if (!playlist_add_rc) {
      enum mpd_error err = mpd_connection_get_error(conn);
      if (err == MPD_ERROR_SERVER) {
        fprintf(stderr, "mpd_send_playlist_add server error code: %d\n", mpd_connection_get_server_error(conn));
        db_entities_free(db_entities, playlist_len);
        return 1;
      }
      fprintf(stderr, "mpd_send_playlist_add error: %d\n", mpd_connection_get_error(conn));
      db_entities_free(db_entities, playlist_len);
      return 1;
    }
    bool res_rc = mpd_response_finish(conn);
    if (!res_rc) {
      enum mpd_error err = mpd_connection_get_error(conn);
      if (err == MPD_ERROR_SERVER) {
        fprintf(stderr, "mpd_response_finish server error code: %d\n", mpd_connection_get_server_error(conn));
        db_entities_free(db_entities, playlist_len);
        return 1;
      }
      fprintf(stderr, "mpd_response_finish error at random_playlist: %d", mpd_connection_get_error(conn));
      db_entities_free(db_entities, playlist_len);
      return 1;
    }
    free(db_entities[i]->path);
    free(db_entities[i]);
  }
  return 0;
}

void db_entities_free(DatabaseEntity **db_entities, size_t len) {
  for (int i = 0; i < len; i++) {
    free(db_entities[i]->path);
    free(db_entities[i]);
  }
}

int add_mpd_db_songs_from_bliss_db(VerifySongController *controller, int mpd_db_id, MPDDatabaseResponse *db_response) {
  int *i = controller->i;
  char *z_err = 0;
  int diff = mpd_db_id - *i;
  while (diff--) {
    char *missing_song_query_header = "select * from song where id = ";
    int len = strlen(missing_song_query_header) + snprintf(NULL, 0, "%d", *i) + 2;
    char *missing_song_query = malloc(len);
    if (missing_song_query == NULL) {
      fprintf(stderr, "Out of memory at verify_song_callback missing_song_query malloc\n");
      free(db_response);
      return 1;
    }
    snprintf(missing_song_query, len, "%s%d;", missing_song_query_header, *i);
    DatabaseEntity *db_entity = malloc(sizeof(DatabaseEntity));
    if (db_entity == NULL) {
      fprintf(stderr, "Out of memory at verify_song_callback missing branch db_entity malloc\n");
      free(db_response);
      return 1;
    }
    db_entity->type = MPD_ENTITY_TYPE_SONG;
    db_entity->song_id = *i;
    db_entity->path = NULL;
    int bliss_rc = sqlite3_exec(controller->bliss_db, missing_song_query, song_query_callback, db_entity, &z_err);
    if (bliss_rc != SQLITE_OK) {
      fprintf(stderr, "sqlite exec error at verify_song_callback: %s\n", z_err);
      free(z_err);
      free(missing_song_query);
      free(db_entity);
      free(db_response);
      return 1;
    }
    if (db_entity->path == NULL) {
      fprintf(stderr, "Failed to find matching song_id for %d, you may need to run `blissify rescan`; continuing...\n", *i);
      free(missing_song_query);
      free(db_entity);
      (*i)++;
      continue;
    }
    if (mpd_search_db_songs(controller->conn, true) &&
        mpd_search_add_uri_constraint(controller->conn, MPD_OPERATOR_DEFAULT, db_entity->path) &&
        mpd_search_commit(controller->conn)) {
      struct mpd_song *song;
      while ((song = mpd_recv_song(controller->conn))) {
        db_entity->atime = mpd_song_get_added(song);
        db_entity->mtime = mpd_song_get_last_modified(song);
        mpd_song_free(song);
      }
    }
    char* bliss_path_prefix = strstr(db_entity->path, MPD_MUSIC_DIR);
    if (bliss_path_prefix == NULL) {
      fprintf(stderr, "Could not remove prefix from bliss path for %s\n", db_entity->path);
      free(missing_song_query);
      free(db_entity->path);
      free(db_entity);
      free(db_response);
      return 1;
    }
    if (MPD_MUSIC_DIR[0] != '\0' && bliss_path_prefix == db_entity->path) { // prefix found
      memmove(db_entity->path, db_entity->path + strlen(MPD_MUSIC_DIR), strlen(db_entity->path + strlen(MPD_MUSIC_DIR)) + 1);
    } 
    db_entity->path = replace_all(db_entity->path, "'", "''");
    if (!add_to_mpd_db(db_entity, controller->mpd_db, controller->bliss_db)) {
      fprintf(stderr, "Failed to add song %s to mpd_db at verify_song_callback\n", db_entity->path);
      free(missing_song_query);
      free(db_entity->path);
      free(db_entity);
      free(db_response);
      return 1;
    }
    else {
      printf("Added song %s to mpd_db from bliss_db\n", db_entity->path);
    }
    free(missing_song_query);
    free(db_entity->path);
    free(db_entity);
    (*i)++;
  }
  return verify_mpd_db_song_against_bliss_db(controller, mpd_db_id, db_response);
}

int verify_mpd_db_song_against_bliss_db(VerifySongController *controller, int mpd_db_id, MPDDatabaseResponse *db_response) {
  char *z_err = 0;
  char *existing_song_query_header = "select * from song where id = ";
  int len = strlen(existing_song_query_header) + snprintf(NULL, 0, "%d", mpd_db_id) + 2;
  char *existing_song_query = malloc(len);
  if (existing_song_query == NULL) {
    fprintf(stderr, "Out of memory at verify_song_callback existing_song_query malloc\n");
    free(db_response);
    return 1;
  }
  snprintf(existing_song_query, len, "%s%d;", existing_song_query_header, mpd_db_id);
  DatabaseEntity *db_entity = malloc(sizeof(DatabaseEntity));
  if (db_entity == NULL) {
    fprintf(stderr, "Out of memory at verify_song_callback existing branch db_entity malloc\n");
    free(db_response);
    return 1;
  }
  db_entity->type = MPD_ENTITY_TYPE_SONG;
  db_entity->song_id = mpd_db_id;
  db_entity->path = NULL;
  int bliss_rc = sqlite3_exec(controller->bliss_db, existing_song_query, song_query_callback, db_entity, &z_err);
  if (bliss_rc != SQLITE_OK) {
    fprintf(stderr, "sqlite exec error at verify_song_callback: %s\n", z_err);
    free(z_err);
    free(existing_song_query);
    free(db_entity);
    free(db_response);
    return 1;
  }
  if (db_entity->path == NULL) {
    fprintf(stderr, "Failed to find matching song_id for %d, you may need to run `blissify rescan`; continuing...\n", mpd_db_id);
    free(existing_song_query);
    free(db_entity);
    free(db_response);
    return 0;
  }
  char* bliss_path_prefix = strstr(db_entity->path, MPD_MUSIC_DIR);
  if (bliss_path_prefix == NULL) {
    fprintf(stderr, "Could not remove prefix from bliss path for %s\n", db_entity->path);
    free(existing_song_query);
    free(db_entity->path);
    free(db_entity);
    free(db_response);
    return 1;
  }
  if (MPD_MUSIC_DIR[0] != '\0' && bliss_path_prefix == db_entity->path) { // prefix found
    memmove(db_entity->path, db_entity->path + strlen(MPD_MUSIC_DIR), strlen(db_entity->path + strlen(MPD_MUSIC_DIR)) + 1);
  }
  int path_col_pos = str_pos_in_arr("path", db_response->col_name, db_response->argc);
  if (path_col_pos == -1) {
    fprintf(stderr, "path column not found at verify_song_callback\n");
    free(existing_song_query);
    free(db_entity->path);
    free(db_entity);
    free(db_response);
    return 1;
  }
  if (strcmp(db_entity->path, db_response->argv[path_col_pos]) != 0) {
    fprintf(stderr, "mpd_db and bliss_db don't match for song_id %d, mpd_db path %s, bliss_db path %s\n", mpd_db_id, db_response->argv[path_col_pos], db_entity->path);
    free(existing_song_query);
    free(db_entity->path);
    free(db_entity);
    free(db_response);
    return 1;
  }
  (*controller->i)++;
  free(existing_song_query);
  free(db_entity->path);
  free(db_entity);
  free(db_response);
  return 0;
}

int verify_mpd_db(struct mpd_connection *conn, sqlite3 *mpd_db, sqlite3 *bliss_db) {
  char *z_err = 0;
  char *all_songs_query = "select * from entities order by song_id;";
  printf("Verifying mpd.db...\n");
  VerifySongController controller = {
    .i = malloc(sizeof(int)),
    .conn = conn,
    .bliss_db = bliss_db,
    .mpd_db = mpd_db
  };
  if (controller.i == NULL) {
    fprintf(stderr, "Out of memory at verify_mpd_db malloc\n");
    return 1;
  }
  *controller.i = 1;
  int mpd_rc = sqlite3_exec(mpd_db, all_songs_query, verify_song_callback, &controller, &z_err);
  if (mpd_rc != SQLITE_OK) {
    fprintf(stderr, "sqlite exec error at verify_mpd_db: %s\n", z_err);
    free(z_err);
    return 1;
  }
  printf("mpd_db verified!\n");
  free(controller.i);
  return 0;
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
  // TODO: check if mpd.db empty and create table entities if so
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
  struct timespec last_ts = { .tv_nsec = 0, .tv_sec = 0 };
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
    snprintf(blissify_cmd, 255, "MPD_HOST='%s@127.0.0.1' MPD_PORT=6600 blissify update", argv[1]); // TODO: get host and port programmatically
  }
  else {
    snprintf(blissify_cmd, 255, "MPD_PORT=6600 blissify update"); // TODO: get host and port programmatically
  }
  printf("Running `blissify update`...\n");
  int blissify_rc = system(blissify_cmd);
  if (blissify_rc != 0) {
    fprintf(stderr, "`blissify update` failed with exit code %d", blissify_rc);
    mpd_connection_free(conn);
    return 1;
  }

  printf("Getting stats...\n");
  struct mpd_stats *stats = mpd_run_stats(conn);
  if (stats == NULL) {
    fprintf(stderr, "Out of memory at mpd_run_stats\n");
    return 1;
  }
  mpd_total = mpd_stats_get_number_of_artists(stats) +
              mpd_stats_get_number_of_albums(stats) +
              mpd_stats_get_number_of_songs(stats);
  mpd_songs_total = mpd_stats_get_number_of_songs(stats);
  mpd_stats_free(stats);

  if (empty_timestamp) {
    get_all_mpd_songs(conn, mpd_db, bliss_db);
  }
  else {
    get_all_songs_since_timestamp(conn, &last_ts, mpd_db, bliss_db);
  }

  verify_mpd_db(conn, mpd_db, bliss_db);

  random_playlist(conn, mpd_db, bliss_db, 50);

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
