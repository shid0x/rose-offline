use std::time::Duration;

use anyhow::bail;
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;

use crate::{
    reader::RoseFileReader,
    types::{Quat4, Vec2, Vec3},
    RoseFile, RoseFileWriter, VfsPathBuf,
};

#[derive(Debug)]
pub struct IfoObject {
    pub object_name: String,
    pub minimap_position: Vec2<u32>,
    pub object_type: u32,
    pub object_id: u32,
    pub warp_id: u16,
    pub event_id: u16,
    pub position: Vec3<f32>,
    pub rotation: Quat4<f32>,
    pub scale: Vec3<f32>,
}

fn read_object(reader: &mut RoseFileReader) -> anyhow::Result<IfoObject> {
    let object_name = reader.read_u8_length_string()?;
    let warp_id = reader.read_u16()?;
    let event_id = reader.read_u16()?;
    let object_type = reader.read_u32()?;
    let object_id = reader.read_u32()?;
    let minimap_pos_x = reader.read_u32()?;
    let minimap_pos_y = reader.read_u32()?;
    let rotation = reader.read_quat4_xyzw_f32()?;
    let position = reader.read_vector3_f32()?;
    let scale = reader.read_vector3_f32()?;

    Ok(IfoObject {
        object_name: String::from(object_name),
        warp_id,
        event_id,
        object_type,
        object_id,
        minimap_position: Vec2 {
            x: minimap_pos_x,
            y: minimap_pos_y,
        },
        rotation,
        position,
        scale,
    })
}

fn write_object(writer: &mut RoseFileWriter, object: &IfoObject) {
    writer.write_u8_length_string(&object.object_name);
    writer.write_u16(object.warp_id);
    writer.write_u16(object.event_id);
    writer.write_u32(object.object_type);
    writer.write_u32(object.object_id);
    writer.write_u32(object.minimap_position.x);
    writer.write_u32(object.minimap_position.y);
    writer.write_f32(object.rotation.x);
    writer.write_f32(object.rotation.y);
    writer.write_f32(object.rotation.z);
    writer.write_f32(object.rotation.w);
    writer.write_f32(object.position.x);
    writer.write_f32(object.position.y);
    writer.write_f32(object.position.z);
    writer.write_f32(object.scale.x);
    writer.write_f32(object.scale.y);
    writer.write_f32(object.scale.z);
}

pub struct IfoMonsterSpawn {
    pub name: String,
    pub id: u32,
    pub count: u32,
}

pub struct IfoMonsterSpawnPoint {
    pub object: IfoObject,
    pub name: String,
    pub basic_spawns: Vec<IfoMonsterSpawn>,
    pub tactic_spawns: Vec<IfoMonsterSpawn>,
    pub interval: u32,
    pub limit_count: u32,
    pub range: u32,
    pub tactic_points: u32,
}

pub struct IfoEffectObject {
    pub object: IfoObject,
    pub effect_path: VfsPathBuf,
}

pub struct IfoEventObject {
    pub object: IfoObject,
    pub quest_trigger_name: String,
    pub script_function_name: String,
}

pub struct IfoSoundObject {
    pub object: IfoObject,
    pub sound_path: VfsPathBuf,
    pub range: u32,
    pub interval: Duration,
}

pub struct IfoNpc {
    pub object: IfoObject,
    pub ai_id: u32,
    pub quest_file_name: String,
}

pub struct IfoFile {
    pub monster_spawns: Vec<IfoMonsterSpawnPoint>,
    pub npcs: Vec<IfoNpc>,
    pub event_objects: Vec<IfoEventObject>,
    pub animated_objects: Vec<IfoObject>,
    pub collision_objects: Vec<IfoObject>,
    pub deco_objects: Vec<IfoObject>,
    pub cnst_objects: Vec<IfoObject>,
    pub effect_objects: Vec<IfoEffectObject>,
    pub sound_objects: Vec<IfoSoundObject>,
    pub water_size: f32,
    pub water_planes: Vec<(Vec3<f32>, Vec3<f32>)>,
    pub warps: Vec<IfoObject>,
    raw_blocks: Vec<IfoRawBlock>,
}

struct IfoRawBlock {
    block_type: u32,
    data: Vec<u8>,
}

impl IfoFile {
    pub fn has_monster_spawn_block(&self) -> bool {
        self.raw_blocks
            .iter()
            .any(|block| block.block_type == BlockType::MonsterSpawn as u32)
    }
}

#[derive(FromPrimitive)]
enum BlockType {
    DeprecatedMapInfo = 0,
    DecoObject = 1,
    Npc = 2,
    CnstObject = 3,
    SoundObject = 4,
    EffectObject = 5,
    AnimatedObject = 6,
    DeprecatedWater = 7,
    MonsterSpawn = 8,
    WaterPlanes = 9,
    Warp = 10,
    CollisionObject = 11,
    EventObject = 12,
}

#[derive(Default, Clone, Copy)]
pub struct IfoReadOptions {
    pub skip_monster_spawns: bool,
    pub skip_npcs: bool,
    pub skip_animated_objects: bool,
    pub skip_collision_objects: bool,
    pub skip_event_objects: bool,
    pub skip_cnst_objects: bool,
    pub skip_deco_objects: bool,
    pub skip_effect_objects: bool,
    pub skip_sound_objects: bool,
    pub skip_water_planes: bool,
    pub skip_warp_objects: bool,
}

impl RoseFile for IfoFile {
    type ReadOptions = IfoReadOptions;
    type WriteOptions = ();

    fn read(
        mut reader: RoseFileReader,
        read_options: &IfoReadOptions,
    ) -> Result<Self, anyhow::Error> {
        let mut monster_spawns = Vec::new();
        let mut npcs = Vec::new();
        let mut event_objects = Vec::new();
        let mut animated_objects = Vec::new();
        let mut collision_objects = Vec::new();
        let mut cnst_objects = Vec::new();
        let mut deco_objects = Vec::new();
        let mut effect_objects = Vec::new();
        let mut sound_objects = Vec::new();
        let mut water_size = 0.0;
        let mut water_planes = Vec::new();
        let mut warps = Vec::new();

        let block_count = reader.read_u32()?;
        let mut block_headers = Vec::with_capacity(block_count as usize);
        for _ in 0..block_count {
            let block_type = reader.read_u32()?;
            let block_offset = reader.read_u32()?;
            block_headers.push((block_type, block_offset));
        }

        let file_len = reader.cursor.get_ref().len() as u32;
        let mut raw_blocks = Vec::with_capacity(block_headers.len());
        for (block_index, &(block_type, block_offset)) in block_headers.iter().enumerate() {
            let block_end = block_headers
                .iter()
                .enumerate()
                .filter(|(index, (_, offset))| *index != block_index && *offset > block_offset)
                .map(|(_, (_, offset))| *offset)
                .min()
                .unwrap_or(file_len);

            raw_blocks.push(IfoRawBlock {
                block_type,
                data: reader.cursor.get_ref()[block_offset as usize..block_end as usize].to_vec(),
            });

            reader.set_position(block_offset as u64);

            match FromPrimitive::from_u32(block_type) {
                Some(BlockType::AnimatedObject) => {
                    if !read_options.skip_animated_objects {
                        let object_count = reader.read_u32()? as usize;
                        animated_objects.reserve_exact(object_count);

                        for _ in 0..object_count {
                            animated_objects.push(read_object(&mut reader)?);
                        }
                    }
                }
                Some(BlockType::CollisionObject) => {
                    if !read_options.skip_collision_objects {
                        let object_count = reader.read_u32()? as usize;
                        collision_objects.reserve_exact(object_count);

                        for _ in 0..object_count {
                            let object = read_object(&mut reader)?;
                            collision_objects.push(object);
                        }
                    }
                }
                Some(BlockType::CnstObject) => {
                    if !read_options.skip_cnst_objects {
                        let object_count = reader.read_u32()? as usize;
                        cnst_objects.reserve_exact(object_count);

                        for _ in 0..object_count {
                            cnst_objects.push(read_object(&mut reader)?);
                        }
                    }
                }
                Some(BlockType::DecoObject) => {
                    if !read_options.skip_deco_objects {
                        let object_count = reader.read_u32()? as usize;
                        cnst_objects.reserve_exact(object_count);

                        for _ in 0..object_count {
                            deco_objects.push(read_object(&mut reader)?);
                        }
                    }
                }
                Some(BlockType::EventObject) => {
                    if !read_options.skip_event_objects {
                        let object_count = reader.read_u32()? as usize;
                        event_objects.reserve_exact(object_count);

                        for _ in 0..object_count {
                            let object = read_object(&mut reader)?;
                            let quest_trigger_name = reader.read_u8_length_string()?;
                            let script_function_name = reader.read_u8_length_string()?;
                            event_objects.push(IfoEventObject {
                                object,
                                quest_trigger_name: String::from(quest_trigger_name),
                                script_function_name: String::from(script_function_name),
                            })
                        }
                    }
                }
                Some(BlockType::Npc) => {
                    if !read_options.skip_npcs {
                        let object_count = reader.read_u32()? as usize;
                        npcs.reserve_exact(object_count);

                        for _ in 0..object_count {
                            let object = read_object(&mut reader)?;
                            let ai_id = reader.read_u32()?;
                            let quest_file_name = reader.read_u8_length_string()?;
                            npcs.push(IfoNpc {
                                object,
                                ai_id,
                                quest_file_name: String::from(quest_file_name),
                            });
                        }
                    }
                }
                Some(BlockType::MonsterSpawn) => {
                    if !read_options.skip_monster_spawns {
                        let object_count = reader.read_u32()? as usize;
                        monster_spawns.reserve_exact(object_count);

                        for _ in 0..object_count {
                            let object = read_object(&mut reader)?;
                            let spawn_name = reader.read_u8_length_string()?;

                            let basic_count = reader.read_u32()?;
                            let mut basic_spawns = Vec::with_capacity(basic_count as usize);
                            for _ in 0..basic_count {
                                let monster_name = reader.read_u8_length_string()?;
                                let monster_id = reader.read_u32()?;
                                let monster_count = reader.read_u32()?;
                                basic_spawns.push(IfoMonsterSpawn {
                                    name: String::from(monster_name),
                                    id: monster_id,
                                    count: monster_count,
                                });
                            }

                            let tactic_count = reader.read_u32()?;
                            let mut tactic_spawns = Vec::with_capacity(tactic_count as usize);
                            for _ in 0..tactic_count {
                                let monster_name = reader.read_u8_length_string()?;
                                let monster_id = reader.read_u32()?;
                                let monster_count = reader.read_u32()?;
                                tactic_spawns.push(IfoMonsterSpawn {
                                    name: String::from(monster_name),
                                    id: monster_id,
                                    count: monster_count,
                                });
                            }

                            let interval = reader.read_u32()?;
                            let limit_count = reader.read_u32()?;
                            let range = reader.read_u32()?;
                            let tactic_points = reader.read_u32()?;
                            monster_spawns.push(IfoMonsterSpawnPoint {
                                object,
                                name: String::from(spawn_name),
                                basic_spawns,
                                tactic_spawns,
                                interval,
                                limit_count,
                                range,
                                tactic_points,
                            });
                        }
                    }
                }
                Some(BlockType::WaterPlanes) => {
                    if !read_options.skip_water_planes {
                        water_size = reader.read_f32()?;

                        let object_count = reader.read_u32()? as usize;
                        water_planes.reserve_exact(object_count);

                        for _ in 0..object_count {
                            let start = reader.read_vector3_f32()?;
                            let end = reader.read_vector3_f32()?;
                            water_planes.push((start, end));
                        }
                    }
                }
                Some(BlockType::Warp) => {
                    if !read_options.skip_warp_objects {
                        let object_count = reader.read_u32()? as usize;
                        warps.reserve_exact(object_count);

                        for _ in 0..object_count {
                            let object = read_object(&mut reader)?;
                            warps.push(object);
                        }
                    }
                }
                Some(BlockType::EffectObject) => {
                    if !read_options.skip_effect_objects {
                        let object_count = reader.read_u32()? as usize;
                        effect_objects.reserve_exact(object_count);

                        for _ in 0..object_count {
                            let object = read_object(&mut reader)?;
                            let effect_path = reader.read_u8_length_string()?;
                            effect_objects.push(IfoEffectObject {
                                object,
                                effect_path: VfsPathBuf::new(&effect_path),
                            })
                        }
                    }
                }
                Some(BlockType::SoundObject) => {
                    if !read_options.skip_sound_objects {
                        let object_count = reader.read_u32()? as usize;
                        sound_objects.reserve_exact(object_count);

                        for _ in 0..object_count {
                            let object = read_object(&mut reader)?;
                            let sound_path = reader.read_u8_length_string()?;
                            let range = reader.read_u32()?;
                            let interval = Duration::from_secs(reader.read_u32()? as u64);
                            sound_objects.push(IfoSoundObject {
                                object,
                                sound_path: VfsPathBuf::new(&sound_path),
                                range,
                                interval,
                            })
                        }
                    }
                }
                Some(BlockType::DeprecatedMapInfo) | Some(BlockType::DeprecatedWater) => {}
                None => {
                    bail!("Invalid block type {}", block_type)
                }
            }
        }

        Ok(IfoFile {
            monster_spawns,
            npcs,
            event_objects,
            animated_objects,
            collision_objects,
            deco_objects,
            cnst_objects,
            effect_objects,
            sound_objects,
            water_size,
            water_planes,
            warps,
            raw_blocks,
        })
    }

    fn write(
        &self,
        writer: &mut RoseFileWriter,
        _options: &Self::WriteOptions,
    ) -> Result<(), anyhow::Error> {
        let needs_monster_spawn_block =
            !self.monster_spawns.is_empty() && !self.has_monster_spawn_block();
        writer.write_u32((self.raw_blocks.len() + usize::from(needs_monster_spawn_block)) as u32);
        let header_start = writer.buffer.len();
        writer.write_padding(
            (self.raw_blocks.len() + usize::from(needs_monster_spawn_block)) as u64 * 8,
        );

        let mut block_headers =
            Vec::with_capacity(self.raw_blocks.len() + usize::from(needs_monster_spawn_block));
        for raw_block in &self.raw_blocks {
            block_headers.push((raw_block.block_type, writer.buffer.len() as u32));

            if raw_block.block_type == BlockType::MonsterSpawn as u32 {
                write_monster_spawn_block(writer, &self.monster_spawns);
            } else {
                writer.buffer.extend_from_slice(&raw_block.data);
            }
        }

        if needs_monster_spawn_block {
            block_headers.push((BlockType::MonsterSpawn as u32, writer.buffer.len() as u32));
            write_monster_spawn_block(writer, &self.monster_spawns);
        }

        for (index, (block_type, block_offset)) in block_headers.iter().enumerate() {
            let header_offset = header_start + index * 8;
            writer.buffer[header_offset..header_offset + 4]
                .copy_from_slice(&block_type.to_le_bytes());
            writer.buffer[header_offset + 4..header_offset + 8]
                .copy_from_slice(&block_offset.to_le_bytes());
        }

        Ok(())
    }
}

fn write_monster_spawn_block(writer: &mut RoseFileWriter, monster_spawns: &[IfoMonsterSpawnPoint]) {
    writer.write_u32(monster_spawns.len() as u32);

    for spawn in monster_spawns {
        write_object(writer, &spawn.object);
        writer.write_u8_length_string(&spawn.name);

        writer.write_u32(spawn.basic_spawns.len() as u32);
        for monster in &spawn.basic_spawns {
            writer.write_u8_length_string(&monster.name);
            writer.write_u32(monster.id);
            writer.write_u32(monster.count);
        }

        writer.write_u32(spawn.tactic_spawns.len() as u32);
        for monster in &spawn.tactic_spawns {
            writer.write_u8_length_string(&monster.name);
            writer.write_u32(monster.id);
            writer.write_u32(monster.count);
        }

        writer.write_u32(spawn.interval);
        writer.write_u32(spawn.limit_count);
        writer.write_u32(spawn.range);
        writer.write_u32(spawn.tactic_points);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_object() -> IfoObject {
        IfoObject {
            object_name: "spawn_obj".to_string(),
            minimap_position: Vec2 { x: 3, y: 4 },
            object_type: 999,
            object_id: 777,
            warp_id: 1,
            event_id: 2,
            position: Vec3 {
                x: 10.0,
                y: 20.0,
                z: 30.0,
            },
            rotation: Quat4 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
            scale: Vec3 {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
        }
    }

    fn sample_ifo_bytes() -> Vec<u8> {
        let mut writer = RoseFileWriter::default();
        writer.write_u32(2);
        writer.write_u32(BlockType::Npc as u32);
        writer.write_u32(20);
        writer.write_u32(BlockType::MonsterSpawn as u32);
        writer.write_u32(24);
        writer.write_u32(0);

        write_monster_spawn_block(
            &mut writer,
            &[IfoMonsterSpawnPoint {
                object: sample_object(),
                name: "spawn_a".to_string(),
                basic_spawns: vec![IfoMonsterSpawn {
                    name: "jelly".to_string(),
                    id: 101,
                    count: 2,
                }],
                tactic_spawns: vec![IfoMonsterSpawn {
                    name: "king".to_string(),
                    id: 102,
                    count: 1,
                }],
                interval: 30,
                limit_count: 9,
                range: 12,
                tactic_points: 100,
            }],
        );

        writer.buffer.to_vec()
    }

    #[test]
    fn ifo_monster_spawns_round_trip_preserves_names_and_values() {
        let ifo = IfoFile::read(
            RoseFileReader::from(sample_ifo_bytes().as_slice()),
            &Default::default(),
        )
        .unwrap();

        let mut writer = RoseFileWriter::default();
        ifo.write(&mut writer, &()).unwrap();
        let reread = IfoFile::read(
            RoseFileReader::from(writer.buffer.as_ref()),
            &Default::default(),
        )
        .unwrap();

        let spawn = &reread.monster_spawns[0];
        assert_eq!(spawn.name, "spawn_a");
        assert_eq!(spawn.basic_spawns[0].name, "jelly");
        assert_eq!(spawn.basic_spawns[0].id, 101);
        assert_eq!(spawn.basic_spawns[0].count, 2);
        assert_eq!(spawn.tactic_spawns[0].name, "king");
        assert_eq!(spawn.interval, 30);
        assert_eq!(spawn.limit_count, 9);
        assert_eq!(spawn.range, 12);
        assert_eq!(spawn.tactic_points, 100);
    }

    #[test]
    fn ifo_monster_spawn_edits_persist_after_write() {
        let mut ifo = IfoFile::read(
            RoseFileReader::from(sample_ifo_bytes().as_slice()),
            &Default::default(),
        )
        .unwrap();
        ifo.monster_spawns[0].range = 44;
        ifo.monster_spawns[0].basic_spawns[0].id = 202;
        ifo.monster_spawns[0].basic_spawns[0].count = 5;

        let mut writer = RoseFileWriter::default();
        ifo.write(&mut writer, &()).unwrap();
        let reread = IfoFile::read(
            RoseFileReader::from(writer.buffer.as_ref()),
            &Default::default(),
        )
        .unwrap();

        assert_eq!(reread.monster_spawns[0].range, 44);
        assert_eq!(reread.monster_spawns[0].basic_spawns[0].id, 202);
        assert_eq!(reread.monster_spawns[0].basic_spawns[0].count, 5);
    }
}
